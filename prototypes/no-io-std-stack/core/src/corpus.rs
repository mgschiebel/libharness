//! False-positive corpus: legitimate pure std code that must NOT be flagged.
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{Cursor, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

pub fn pure_corpus() -> String {
    let mut s = String::new();
    write!(s, "{}", 1).unwrap();
    let mut buf: Vec<u8> = Vec::new();
    buf.write_all(b"hello").unwrap(); // io::Write into memory
    let mut c = Cursor::new(buf);
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap(); // io::Read from memory
    let _ = std::io::read_to_string(Cursor::new("x")).unwrap();
    let _ = std::io::copy(&mut Cursor::new("a"), &mut std::io::sink());
    let _e = std::io::Error::other("x");
    let ip: IpAddr = Ipv4Addr::new(127, 0, 0, 1).into();
    let _sa = SocketAddr::new(ip, 80);
    let _p: PathBuf = Path::new("a").join("b"); // pure path manipulation
    let _ext = _p.extension();
    let _d = Duration::from_millis(5) + Duration::from_secs(1);
    let (tx, rx) = mpsc::channel::<u8>();
    tx.send(1).unwrap();
    let _ = rx.recv();
    let _m = Arc::new(Mutex::new(BTreeMap::<u8, u8>::new()));
    let _r = RwLock::new(HashSet::<u8>::new());
    let _q: VecDeque<BTreeSet<u8>> = VecDeque::new();
    thread_local! { static X: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
    X.with(|x| x.set(1));
    let _ = std::env::consts::OS; // const, not env access
    let _ = format!("{:?}", std::any::type_name::<u8>());
    s + &out
}
