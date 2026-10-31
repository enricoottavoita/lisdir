//! Other PCs: search the files of your other computers that run lisdir, and let them search this one.
//!
//! Off unless switched on in the settings: until then nothing is started, no thread, no connection. Each PC has a
//! key (`peer.key`); its public half is the "code" of that PC. Only PCs in the trusted list can ask anything, and
//! every connection is encrypted and tied to that key (iroh/QUIC: a direct path when the networks allow it, a relay
//! that only sees encrypted bytes when they do not). A question is the search text; the answer is at most 20 names,
//! paths and sizes. Files are never sent and cannot be opened from afar.
//!
//! Pairing: on PC A press "Allow a new PC" (open for two minutes); on PC B paste A's code and press "Pair". A shows
//! who is asking, with the start and end of its code, and the person at A confirms. Both PCs also show a 4-character
//! verification code (from both keys and a random number that only B and A know); A's person confirms only if it is
//! the same on both screens, so a stranger who knows A's code and chooses a familiar name is caught. Then each PC has
//! the other in its list. A PC that is not in the list and is not being paired is closed on at once.
use crate::service::Hit;
use iroh::{endpoint::{presets, Connection}, Endpoint, EndpointId, SecretKey};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex, OnceLock, RwLock},
    time::{Duration, Instant},
};
use tokio::{sync::Mutex, time::{sleep, timeout}};

const ALPN: &[u8] = b"lisdir/search/1";
const MAX_QUERY: usize = 512;
const MAX_REPLY: usize = 256 * 1024;
/// The first contact can take a few seconds (the other PC's address has to be looked up); later ones are instant.
const CONNECT: Duration = Duration::from_secs(10);
const ASK: Duration = Duration::from_secs(3);
/// After a PC did not answer, do not ask it again for this long: typing must never wait on a PC that is off.
const BACKOFF: Duration = Duration::from_secs(10);
/// How long "Allow a new PC" stays open, and how long the person at that PC has to answer a request.
const PAIR_OPEN: Duration = Duration::from_secs(120);
const PAIR_ANSWER: Duration = Duration::from_secs(60);

/// What this PC answers with for a search text (files only).
pub type Search = Arc<dyn Fn(&str) -> Vec<Hit> + Send + Sync>;
/// Called with the name and code of a PC that was just paired, to save it in the settings.
pub type Paired = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Default)]
struct Link {
    conn: Option<Connection>,
    down_until: Option<Instant>,
}

/// The PCs this one trusts: they may ask, and they are asked.
struct Trust {
    list: RwLock<Vec<(String, EndpointId)>>,
    name: String,
    /// this PC's own id
    me: EndpointId,
    /// when each PC last answered us or asked us something
    seen: StdMutex<HashMap<EndpointId, Instant>>,
}

impl Trust {
    fn has(&self, id: &EndpointId) -> bool {
        self.list.read().unwrap().iter().any(|p| p.1 == *id)
    }

    fn saw(&self, id: EndpointId) {
        self.seen.lock().unwrap().insert(id, Instant::now());
    }

    fn add(&self, name: &str, id: EndpointId) {
        let mut l = self.list.write().unwrap();
        l.retain(|p| p.1 != id);
        l.push((name.to_string(), id));
    }
}

struct Peers {
    ep: Endpoint,
    trust: Arc<Trust>,
    links: StdMutex<HashMap<EndpointId, Arc<Mutex<Link>>>>,
}

static PEERS: OnceLock<Arc<Peers>> = OnceLock::new();
static PAIRED: OnceLock<Paired> = OnceLock::new();

fn key_file() -> std::path::PathBuf {
    crate::engine::data_dir().join("peer.key")
}

/// Windows' own data protection (DPAPI): what `protect` seals only comes back for the same Windows account on this
/// PC, so a copy of the file made from a backup, a disk or a shared folder is of no use elsewhere.
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB}};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
    unsafe {
        // flag 1 = CRYPTPROTECT_UI_FORBIDDEN: never ask anything
        let ok = if protect {
            CryptProtectData(&input, null(), null(), null(), null(), 1, &mut out)
        } else {
            CryptUnprotectData(&input, null_mut(), null(), null(), null(), 1, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(v)
    }
}

/// Write the key sealed. If sealing fails the key is kept as it is rather than lost: the PC's code must not change.
fn save_secret(k: &[u8; 32]) {
    let _ = std::fs::create_dir_all(crate::engine::data_dir());
    let _ = std::fs::write(key_file(), dpapi(k, true).unwrap_or_else(|| k.to_vec()));
}

/// The key of this PC. The file holds it sealed with DPAPI; 0.17.0 and 0.17.1 wrote it as 32 plain bytes, which is
/// read once and sealed on the spot (same key, so the PC's code and its pairings stay valid).
fn secret() -> SecretKey {
    // read (or made) once: the settings window and the network start both ask for it at startup, and two callers
    // finding no file would each make a different key
    static KEY: OnceLock<SecretKey> = OnceLock::new();
    KEY.get_or_init(load_secret).clone()
}

fn load_secret() -> SecretKey {
    if let Ok(file) = std::fs::read(key_file()) {
        let plain = <[u8; 32]>::try_from(file.as_slice()).ok();
        if let Some(k) = plain.or_else(|| dpapi(&file, false).and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())) {
            if file.len() == 32 {
                save_secret(&k);
            }
            return SecretKey::from_bytes(&k);
        }
    }
    let k = SecretKey::generate();
    save_secret(&k.to_bytes());
    k
}

/// This PC's code, to give to the other PC.
pub fn my_code() -> String {
    secret().public().to_string()
}

/// The name this PC gives itself when pairing.
pub fn my_name() -> String {
    clean_name(&std::env::var("COMPUTERNAME").unwrap_or_default())
}

/// The 4 characters both people compare when pairing: from the id of the PC that is asked, the id of the PC that
/// asks, and a random number the asking PC chose (so nobody can prepare a matching key in advance). No 0/O/1/I.
fn verify_code(asked: &EndpointId, asking: &EndpointId, nonce: &str) -> String {
    const ABC: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let h = Sha256::new().chain_update(asked.to_string()).chain_update(asking.to_string()).chain_update(nonce).finalize();
    h[..4].iter().map(|&b| ABC[b as usize % 32] as char).collect()
}

/// 16 random hex characters (from a fresh key: the key generator is the random source already at hand).
fn new_nonce() -> String {
    SecretKey::generate().to_bytes().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A name from another PC is text to show, nothing more: no separators, no control characters, short.
fn clean_name(n: &str) -> String {
    let n: String = n.chars().filter(|c| !c.is_control() && *c != '|').take(30).collect();
    let n = n.trim();
    if n.is_empty() { "PC".into() } else { n.into() }
}

/// `Name|code` (or just the code) to a name and an id.
pub fn parse_peer(line: &str) -> Option<(String, EndpointId)> {
    let (name, code) = line.rsplit_once('|').unwrap_or(("", line));
    let id: EndpointId = code.trim().parse().ok()?;
    let name = name.trim();
    Some((if name.is_empty() { format!("PC {}", &code.trim()[..6]) } else { name.to_string() }, id))
}

/// Answer lines: `is_dir \t size \t name \t path`.
fn format_reply(hits: &[Hit]) -> String {
    hits.iter().take(20).map(|h| format!("{}\t{}\t{}\t{}\n", h.is_dir as u8, h.size, h.name, h.path)).collect()
}

fn parse_reply(text: &str, peer: &str) -> Vec<Hit> {
    text.lines()
        .take(20)
        .filter_map(|l| {
            let mut p = l.splitn(4, '\t');
            let (dir, size, name, path) = (p.next()?, p.next()?, p.next()?, p.next()?);
            let is_dir = dir == "1";
            let mut h = Hit::new(name.into(), path.into(), is_dir, 0.0).with_size(size.parse().ok()?);
            // the file is not on this PC: an icon by extension only, never by path
            h.icon = match name.rsplit_once('.') {
                _ if is_dir => "dir".into(),
                Some((s, e)) if !s.is_empty() && !e.is_empty() => format!(".{}", e.to_lowercase()),
                _ => "file".into(),
            };
            h.peer = peer.into();
            Some(h)
        })
        .collect()
}

// ---------------------------------------------------------------- pairing state (shared with the settings window)

#[derive(Default)]
struct Pairing {
    /// "Allow a new PC" is open until then
    until: Option<Instant>,
    /// a PC that asked and waits for an answer: its name, its code and the verification code
    request: Option<(String, EndpointId, String)>,
    answer: Option<bool>,
}

static PAIRING: StdMutex<Pairing> = StdMutex::new(Pairing { until: None, request: None, answer: None });

/// The verification code of the pairing this PC started (shown while it waits), and the last PC paired here.
static OUTGOING: StdMutex<String> = StdMutex::new(String::new());
static LAST_PAIRED: StdMutex<String> = StdMutex::new(String::new());

pub struct PairState {
    pub open: bool,
    pub name: String,
    pub code: String,
    /// the 4-character verification code of the pairing in progress (either side), empty if none
    pub check: String,
    /// name of the last PC that was paired with this one since lisdir started, empty if none
    pub done: String,
}

/// The async runtime, built the first time it is needed: with the option off it never starts.
pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime"))
}

/// Is the feature running (so that pairing can work)?
pub fn running() -> bool {
    PEERS.get().is_some()
}

/// Let one new PC ask to be paired for the next two minutes. False when the feature is not running.
pub fn open_pairing() -> bool {
    if !running() {
        return false;
    }
    *PAIRING.lock().unwrap() = Pairing { until: Some(Instant::now() + PAIR_OPEN), ..Pairing::default() };
    true
}

pub fn pairing_state() -> PairState {
    let p = PAIRING.lock().unwrap();
    let (name, code, check) = p.request.as_ref().map_or_else(Default::default, |(n, id, c)| (n.clone(), id.to_string(), c.clone()));
    let check = if check.is_empty() { OUTGOING.lock().unwrap().clone() } else { check };
    let done = LAST_PAIRED.lock().unwrap().clone();
    PairState { open: p.until.is_some_and(|t| t > Instant::now()), name, code, check, done }
}

/// The person's answer to the request on screen.
pub fn answer_pairing(accept: bool) {
    let mut p = PAIRING.lock().unwrap();
    if p.request.is_some() {
        p.answer = Some(accept);
    }
}

pub fn close_pairing() {
    *PAIRING.lock().unwrap() = Pairing::default();
}

/// A new PC asks to be paired: wait for the person here to answer. Only one request at a time, only while open.
async fn pair_request(name: &str, id: EndpointId, check: String) -> bool {
    {
        let mut p = PAIRING.lock().unwrap();
        if !p.until.is_some_and(|t| t > Instant::now()) || p.request.is_some() {
            return false;
        }
        (p.request, p.answer) = (Some((name.to_string(), id, check)), None);
    }
    let end = Instant::now() + PAIR_ANSWER;
    let mut accepted = false;
    while Instant::now() < end {
        sleep(Duration::from_millis(200)).await;
        let a = PAIRING.lock().unwrap().answer;
        if let Some(a) = a {
            accepted = a;
            break;
        }
    }
    let mut p = PAIRING.lock().unwrap();
    (p.request, p.answer) = (None, None);
    if accepted {
        p.until = None; // one PC per opening
    }
    accepted
}

/// Ask the PC with this code to pair with us. Its owner has to confirm there, so this can wait a while.
async fn pair_over(ep: &Endpoint, trust: &Trust, code: &str) -> Result<String, String> {
    let id: EndpointId = code.trim().parse().map_err(|_| "that is not a valid code".to_string())?;
    if id == ep.id() {
        return Err("that is the code of this PC".into());
    }
    let nonce = new_nonce();
    *OUTGOING.lock().unwrap() = verify_code(&id, &ep.id(), &nonce);
    let result = pair_talk(ep, trust, id, &nonce).await;
    OUTGOING.lock().unwrap().clear();
    result
}

async fn pair_talk(ep: &Endpoint, trust: &Trust, id: EndpointId, nonce: &str) -> Result<String, String> {
    let conn = timeout(CONNECT, ep.connect(id, ALPN)).await.map_err(|_| "no answer from that PC")?.map_err(|_| "cannot reach that PC")?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
    send.write_all(format!("PAIR\t{}\t{nonce}", trust.name).as_bytes()).await.map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())?;
    let reply = timeout(PAIR_ANSWER + Duration::from_secs(10), recv.read_to_end(MAX_QUERY)).await.map_err(|_| "no answer from that PC")?.map_err(|_| "that PC refused")?;
    let reply = String::from_utf8_lossy(&reply).into_owned();
    let name = reply.strip_prefix("OK\t").map(clean_name).ok_or("that PC refused, or is not waiting for a new PC")?;
    trust.add(&name, id);
    conn.close(0u32.into(), b"paired");
    Ok(name)
}

/// Pair with the PC that has this code (the settings window's "Pair" button). Returns the name of that PC.
pub async fn pair(code: String) -> Result<String, String> {
    let peers = PEERS.get().ok_or("switch the option on and restart lisdir first")?.clone();
    let name = pair_over(&peers.ep, &peers.trust, &code).await?;
    if let (Some(done), Ok(id)) = (PAIRED.get(), code.trim().parse::<EndpointId>()) {
        done(&name, &id.to_string());
    }
    Ok(name)
}

// ---------------------------------------------------------------- serving and asking

/// Start sharing this PC and asking the others. `peers` is the list of trusted PCs.
pub fn start(peers: Vec<(String, EndpointId)>, search: Search, paired: Paired) {
    let _ = PAIRED.set(paired);
    rt().spawn(async move {
        let Ok(ep) = Endpoint::builder(presets::N0).secret_key(secret()).alpns(vec![ALPN.to_vec()]).bind().await else { return };
        let trust = Arc::new(Trust { list: RwLock::new(peers), name: my_name(), me: ep.id(), seen: StdMutex::default() });
        let peers = Arc::new(Peers { ep: ep.clone(), trust: trust.clone(), links: StdMutex::default() });
        let _ = PEERS.set(peers.clone());
        // connect now, so the first search already has its connections
        for (name, id) in trust.list.read().unwrap().clone() {
            let peers = peers.clone();
            rt().spawn(async move { ask(&peers, &name, id, " ").await });
        }
        while let Some(incoming) = ep.accept().await {
            let (trust, search) = (trust.clone(), search.clone());
            rt().spawn(async move {
                if let Ok(conn) = incoming.await {
                    serve(conn, trust, search).await;
                }
            });
        }
    });
}

/// Replace the list of trusted PCs (after the settings were saved).
pub fn set_peers(list: Vec<(String, EndpointId)>) {
    if let Some(p) = PEERS.get() {
        *p.trust.list.write().unwrap() = list;
    }
}

async fn serve(conn: Connection, trust: Arc<Trust>, search: Search) {
    let who = conn.remote_id();
    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
        let Ok(msg) = recv.read_to_end(MAX_QUERY).await else { return };
        let msg = String::from_utf8_lossy(&msg).into_owned();
        let reply = if let Some(rest) = msg.strip_prefix("PAIR\t") {
            let (name, nonce) = rest.split_once('\t').unwrap_or((rest, ""));
            let name = clean_name(name);
            // a request without the random number (an older lisdir) cannot be verified: refused
            let valid = (8..=32).contains(&nonce.len()) && nonce.bytes().all(|b| b.is_ascii_hexdigit());
            if trust.has(&who) || (valid && pair_request(&name, who, verify_code(&trust.me, &who, nonce)).await) {
                if !trust.has(&who) {
                    trust.add(&name, who);
                    *LAST_PAIRED.lock().unwrap() = name.clone();
                    if let Some(done) = PAIRED.get() {
                        done(&name, &who.to_string());
                    }
                }
                format!("OK\t{}", trust.name)
            } else {
                "NO".into()
            }
        } else if trust.has(&who) {
            trust.saw(who);
            let s = search.clone();
            let hits = rt().spawn_blocking(move || s(&msg)).await.unwrap_or_default();
            format_reply(&hits)
        } else {
            conn.close(1u32.into(), b"unknown PC");
            return;
        };
        if send.write_all(reply.as_bytes()).await.is_err() || send.finish().is_err() {
            return;
        }
    }
}

async fn ask(peers: &Peers, name: &str, id: EndpointId, q: &str) -> Option<Vec<Hit>> {
    let link = peers.links.lock().unwrap().entry(id).or_default().clone();
    // a connection attempt is already under way (or another question is in flight): do not queue behind it
    let mut link = link.try_lock().ok()?;
    if link.down_until.is_some_and(|t| t > Instant::now()) {
        return None;
    }
    let result = async {
        if link.conn.is_none() {
            link.conn = Some(timeout(CONNECT, peers.ep.connect(id, ALPN)).await.ok()?.ok()?);
        }
        let (mut send, mut recv) = link.conn.as_ref()?.open_bi().await.ok()?;
        send.write_all(q.as_bytes()).await.ok()?;
        send.finish().ok()?;
        let body = timeout(ASK, recv.read_to_end(MAX_REPLY)).await.ok()?.ok()?;
        Some(parse_reply(&String::from_utf8_lossy(&body), name))
    }
    .await;
    match result {
        Some(_) => peers.trust.saw(id),
        None => (link.conn, link.down_until) = (None, Some(Instant::now() + BACKOFF)),
    }
    result
}

pub struct PeerState {
    pub name: String,
    /// a working connection to it right now
    pub online: bool,
    /// seconds since it last answered or asked; None if never since lisdir started
    pub seen: Option<u64>,
}

/// The trusted PCs and whether each is connected (for the settings window). A PC that is not connected is tried
/// again in the background, at most once per backoff period, so the list catches up when it comes on.
pub fn status() -> Vec<PeerState> {
    let Some(peers) = PEERS.get().cloned() else { return vec![] };
    let list = peers.trust.list.read().unwrap().clone();
    list.into_iter()
        .map(|(name, id)| {
            let link = peers.links.lock().unwrap().get(&id).cloned();
            // busy = a question or a connection attempt is in flight: say connected only if that is already known
            let online = link.is_some_and(|l| l.try_lock().is_ok_and(|l| l.conn.as_ref().is_some_and(|c| c.close_reason().is_none())));
            if !online {
                let (peers, name) = (peers.clone(), name.clone());
                rt().spawn(async move { ask(&peers, &name, id, " ").await });
            }
            let seen = peers.trust.seen.lock().unwrap().get(&id).map(|t| t.elapsed().as_secs());
            PeerState { name, online, seen }
        })
        .collect()
}

/// Ask every trusted PC at once; the ones that are off or slow are left out.
pub async fn search(q: String) -> Vec<Hit> {
    let Some(peers) = PEERS.get().cloned() else { return vec![] };
    let q = q.replace(['\t', '\n', '\r'], " ");
    let list = peers.trust.list.read().unwrap().clone();
    let tasks: Vec<_> = list
        .into_iter()
        .map(|(name, id)| {
            let (peers, q) = (peers.clone(), q.clone());
            rt().spawn(async move { ask(&peers, &name, id, &q).await.unwrap_or_default() })
        })
        .collect();
    let mut all = vec![];
    for t in tasks {
        all.extend(t.await.unwrap_or_default());
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_are_parsed_from_the_settings_line() {
        let code = SecretKey::generate().public().to_string();
        let (name, id) = parse_peer(&format!("Office|{code}")).unwrap();
        assert_eq!((name.as_str(), id.to_string()), ("Office", code.clone()));
        assert!(parse_peer(&code).unwrap().0.starts_with("PC "));
        assert!(parse_peer("Office|not a code").is_none() && parse_peer("").is_none());
    }

    #[test]
    fn verify_code_is_four_clear_characters_that_change_with_every_input() {
        let (a, b) = (SecretKey::from_bytes(&[1; 32]).public(), SecretKey::from_bytes(&[2; 32]).public());
        let c = verify_code(&a, &b, "00112233aabbccdd");
        assert!(c.len() == 4 && c.chars().all(|x| x.is_ascii_alphanumeric() && !"01OI".contains(x)), "{c}");
        assert_eq!(c, verify_code(&a, &b, "00112233aabbccdd"));
        assert_ne!(c, verify_code(&a, &b, "00112233aabbccde"));
        assert_ne!(c, verify_code(&b, &a, "00112233aabbccdd"));
        assert_eq!(new_nonce().len(), 16);
    }

    #[test]
    fn the_key_is_sealed_with_dpapi_and_comes_back() {
        let key = [7u8; 32];
        let sealed = dpapi(&key, true).expect("DPAPI works for the logged-in user");
        assert!(sealed.len() > 32 && !sealed.windows(32).any(|w| w == key), "the sealed file does not hold the key as it is");
        assert_eq!(dpapi(&sealed, false).unwrap(), key);
        assert!(dpapi(&key, false).is_none(), "plain bytes are not a sealed blob");
    }

    #[test]
    fn names_from_other_pcs_are_made_safe() {
        assert_eq!(clean_name("Home|PC\n\t"), "HomePC");
        assert_eq!(clean_name(" \n"), "PC");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 30);
    }

    #[test]
    fn reply_round_trip_keeps_icons_by_extension() {
        let hits = vec![
            Hit::new("a.exe".into(), r"C:\x\a.exe".into(), false, 9.0).with_size(5),
            Hit::new("docs".into(), r"C:\docs".into(), true, 1.0).with_size(7),
        ];
        let back = parse_reply(&format_reply(&hits), "Home");
        assert_eq!(back.len(), 2);
        assert_eq!((back[0].name.as_str(), back[0].size, back[0].icon.as_str(), back[0].peer.as_str()), ("a.exe", 5, ".exe", "Home"));
        assert_eq!((back[1].is_dir, back[1].icon.as_str()), (true, "dir"));
        assert!(parse_reply("garbage\nx\ty\n", "p").is_empty());
    }

    async fn bind(k: SecretKey) -> Endpoint {
        Endpoint::builder(presets::Minimal).secret_key(k).alpns(vec![ALPN.to_vec()]).bind().await.unwrap()
    }

    async fn ask_once(conn: &Connection, text: &[u8]) -> Result<Vec<u8>, std::io::Error> {
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(text).await.map_err(std::io::Error::other)?;
        send.finish().map_err(std::io::Error::other)?;
        recv.read_to_end(MAX_REPLY).await.map_err(std::io::Error::other)
    }

    /// Two PCs in one process over loopback with no relay: only a listed PC gets answers, a stranger is refused, and
    /// pairing (opened on one side, confirmed by its person) puts each PC in the other's list.
    #[test]
    fn answers_only_to_listed_pcs_and_pairing_lists_both() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (ks, kc, ke) = (SecretKey::generate(), SecretKey::generate(), SecretKey::generate());
            let (server, client, stranger) = (bind(ks).await, bind(kc.clone()).await, bind(ke.clone()).await);
            let trust_s = Arc::new(Trust { list: RwLock::new(vec![("Client".into(), kc.public())]), name: "Server".into(), me: server.id(), seen: StdMutex::default() });
            let trust_e = Trust { list: RwLock::new(vec![]), name: "Stranger".into(), me: stranger.id(), seen: StdMutex::default() };
            let search: Search = Arc::new(|q| vec![Hit::new(format!("{q}.txt"), format!(r"C:\{q}.txt"), false, 1.0).with_size(3)]);
            let addr = server.addr();
            let (srv, t, s) = (server.clone(), trust_s.clone(), search.clone());
            tokio::spawn(async move {
                while let Some(inc) = srv.accept().await {
                    let (t, s) = (t.clone(), s.clone());
                    tokio::spawn(async move { serve(inc.await.unwrap(), t, s).await });
                }
            });
            // a listed PC
            let conn = client.connect(addr.clone(), ALPN).await.unwrap();
            let hits = parse_reply(&String::from_utf8(ask_once(&conn, b"hello").await.unwrap()).unwrap(), "S");
            assert_eq!((hits[0].name.as_str(), hits[0].size), ("hello.txt", 3));
            assert!(trust_s.seen.lock().unwrap().contains_key(&kc.public()), "a question marks the PC as seen");
            // a stranger: no answer to a question, and no pairing while "Allow a new PC" is closed
            let conn = stranger.connect(addr.clone(), ALPN).await.unwrap();
            assert!(ask_once(&conn, b"hello").await.is_err(), "a PC that is not in the list must get no answer");
            let conn = stranger.connect(addr.clone(), ALPN).await.unwrap();
            assert_eq!(ask_once(&conn, b"PAIR\tStranger").await.unwrap(), b"NO");
            assert!(!trust_s.has(&ke.public()));
            // pairing: opened on the server side (the global state), confirmed by its "person"
            *PAIRING.lock().unwrap() = Pairing { until: Some(Instant::now() + PAIR_OPEN), ..Pairing::default() };
            let person = tokio::spawn(async {
                loop {
                    sleep(Duration::from_millis(50)).await;
                    let st = pairing_state();
                    if !st.name.is_empty() {
                        assert_eq!(st.name, "Stranger");
                        assert_eq!(st.check.len(), 4, "the person is shown a verification code");
                        assert_eq!(st.check, *OUTGOING.lock().unwrap(), "both PCs show the same verification code");
                        return answer_pairing(true);
                    }
                }
            });
            let name = pair_over(&stranger, &trust_e, &server.id().to_string()).await.unwrap();
            person.await.unwrap();
            assert_eq!(name, "Server");
            assert!(trust_s.has(&ke.public()) && trust_e.has(&server.id()), "each PC lists the other");
            // and the new PC is now answered; the opening is used up
            let conn = stranger.connect(addr.clone(), ALPN).await.unwrap();
            assert!(ask_once(&conn, b"x").await.is_ok());
            assert!(!pairing_state().open);
            assert_eq!(pairing_state().done, "Stranger", "the pairing is reported");
            assert!(pair_over(&stranger, &trust_e, "garbage").await.is_err());
        });
    }
}
