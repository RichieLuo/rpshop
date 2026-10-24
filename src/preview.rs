//! Per-editor, loopback-only prototype preview. No files are served or written.

use crate::{export, model::Project};
use anyhow::{Context, Result, anyhow, ensure};
use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const IO_SLICE: Duration = Duration::from_millis(200);
const HEADER_DEADLINE: Duration = Duration::from_secs(2);
const WRITE_DEADLINE: Duration = Duration::from_secs(5);

/// A single application's preview listener. Dropping it closes the listener and
/// joins its worker; a later editor session gets a new port and session path.
pub struct PreviewServer {
    address: SocketAddr,
    path: String,
    snapshot: Arc<RwLock<Arc<Snapshot>>>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

struct Snapshot {
    project: Project,
    page_keys: BTreeMap<u64, String>,
    native: NativeSnapshot,
}

enum NativeSnapshot {
    None,
    Prepared(Arc<crate::rp_format::NativePreview>),
    Document {
        source: Arc<crate::rp_format::NativeDocument>,
        compiled: OnceLock<std::result::Result<Arc<crate::rp_format::NativePreview>, String>>,
    },
}

impl Snapshot {
    fn new(project: Project, page_keys: BTreeMap<u64, String>) -> Result<Self> {
        project.validate().context("预览工程无效")?;
        let page_ids = project
            .pages
            .iter()
            .map(|page| page.id)
            .collect::<BTreeSet<_>>();
        ensure!(
            page_keys.keys().all(|id| page_ids.contains(id)),
            "预览页面标识引用了不存在的页面"
        );
        let mut unique = BTreeSet::new();
        for page in &project.pages {
            let key = page_keys
                .get(&page.id)
                .cloned()
                .unwrap_or_else(|| format!("id-{}", page.id));
            ensure!(
                !key.is_empty() && key.len() <= 1024 && unique.insert(key),
                "预览页面标识不能为空、重复或超过 1024 字节"
            );
        }
        Ok(Self {
            project,
            page_keys,
            native: NativeSnapshot::None,
        })
    }

    fn native_preview(&self) -> Result<Option<&crate::rp_format::NativePreview>> {
        match &self.native {
            NativeSnapshot::None => Ok(None),
            NativeSnapshot::Prepared(preview) => Ok(Some(preview)),
            NativeSnapshot::Document { source, compiled } => {
                match compiled.get_or_init(|| {
                    source
                        .preview_scene(&self.project)
                        .map(Arc::new)
                        .map_err(|error| format!("{error:#}"))
                }) {
                    Ok(preview) => Ok(Some(preview)),
                    Err(message) => Err(anyhow!("原生预览构建失败：{message}")),
                }
            }
        }
    }
}

impl PreviewServer {
    pub fn start(project: Project) -> Result<Self> {
        Self::start_with_page_keys(project, BTreeMap::new())
    }

    /// Bind native logical page identities independently of editor numeric IDs.
    pub fn start_with_page_keys(
        project: Project,
        page_keys: BTreeMap<u64, String>,
    ) -> Result<Self> {
        Self::start_with_native(project, page_keys, None)
    }

    pub fn start_with_native(
        project: Project,
        page_keys: BTreeMap<u64, String>,
        native: Option<Arc<crate::rp_format::NativePreview>>,
    ) -> Result<Self> {
        let mut snapshot = Snapshot::new(project, page_keys)?;
        snapshot.native = native.map_or(NativeSnapshot::None, NativeSnapshot::Prepared);
        Self::start_snapshot(snapshot)
    }

    /// Capture a read-only native document. The HTTP worker builds its runtime
    /// scene on the first refresh, once for this snapshot, without editor locks.
    pub fn start_with_native_document(
        project: Project,
        page_keys: BTreeMap<u64, String>,
        native: Option<Arc<crate::rp_format::NativeDocument>>,
    ) -> Result<Self> {
        let mut snapshot = Snapshot::new(project, page_keys)?;
        snapshot.native = native.map_or(NativeSnapshot::None, |source| NativeSnapshot::Document {
            source,
            compiled: OnceLock::new(),
        });
        Self::start_snapshot(snapshot)
    }

    fn start_snapshot(snapshot: Snapshot) -> Result<Self> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("无法启动本机原型预览")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let path = session_path();
        let snapshot = Arc::new(RwLock::new(Arc::new(snapshot)));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_stopping = Arc::clone(&stopping);
        let worker_path = path.clone();
        let worker = thread::Builder::new()
            .name("rpshop-preview".into())
            .spawn(move || {
                while !worker_stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            if worker_stopping.load(Ordering::Acquire) {
                                break;
                            }
                            let _ = serve(
                                &mut stream,
                                &worker_path,
                                &worker_snapshot,
                                &worker_stopping,
                            );
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            })
            .context("无法创建本机预览线程")?;
        Ok(Self {
            address,
            path,
            snapshot,
            stopping,
            worker: Some(worker),
        })
    }

    /// Replace the next refresh's snapshot. Existing browser content remains
    /// unchanged until refresh, and a response in progress keeps its snapshot.
    pub fn publish(&mut self, project: Project) -> Result<()> {
        self.publish_with_page_keys(project, BTreeMap::new())
    }

    /// Replace document content and its logical page identities atomically.
    pub fn publish_with_page_keys(
        &mut self,
        project: Project,
        page_keys: BTreeMap<u64, String>,
    ) -> Result<()> {
        self.publish_with_native(project, page_keys, None)
    }

    pub fn publish_with_native(
        &mut self,
        project: Project,
        page_keys: BTreeMap<u64, String>,
        native: Option<Arc<crate::rp_format::NativePreview>>,
    ) -> Result<()> {
        let mut snapshot = Snapshot::new(project, page_keys).context("保留上次预览快照")?;
        snapshot.native = native.map_or(NativeSnapshot::None, NativeSnapshot::Prepared);
        self.publish_snapshot(snapshot)
    }

    /// Publish editor changes without compiling their native runtime on the UI
    /// thread. Previous requests retain their independently cached snapshot.
    pub fn publish_with_native_document(
        &mut self,
        project: Project,
        page_keys: BTreeMap<u64, String>,
        native: Option<Arc<crate::rp_format::NativeDocument>>,
    ) -> Result<()> {
        let mut snapshot = Snapshot::new(project, page_keys).context("保留上次预览快照")?;
        snapshot.native = native.map_or(NativeSnapshot::None, |source| NativeSnapshot::Document {
            source,
            compiled: OnceLock::new(),
        });
        self.publish_snapshot(snapshot)
    }

    fn publish_snapshot(&mut self, snapshot: Snapshot) -> Result<()> {
        *self
            .snapshot
            .write()
            .map_err(|_| anyhow!("本机预览快照不可用"))? = Arc::new(snapshot);
        Ok(())
    }

    /// The fragment chooses the page in the player; it is never sent over HTTP.
    pub fn url(&self, page_id: u64) -> String {
        let mut url = format!("http://{}{}#page-{page_id}", self.address, self.path);
        if let Some(key) = self
            .snapshot
            .read()
            .ok()
            .and_then(|snapshot| snapshot.page_keys.get(&page_id).cloned())
        {
            url.push_str("&key=");
            url.push_str(&encode_fragment_value(&key));
        }
        url
    }
}

fn encode_fragment_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}

impl Drop for PreviewServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // Wake a waiting accept loop. Active reads/writes also check the stop
        // flag at bounded intervals, so a stalled browser cannot hold Drop open.
        let _ = TcpStream::connect_timeout(&self.address, IO_SLICE);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn session_path() -> String {
    // RandomState obtains randomized hash keys from the standard library's OS
    // entropy source. Independent states make a fresh opaque 128-bit URL path.
    let first = RandomState::new().build_hasher().finish();
    let second = RandomState::new().build_hasher().finish();
    format!("/{first:016x}{second:016x}/")
}

enum Header {
    Complete(Vec<u8>),
    Closed,
    TooLarge,
    TimedOut,
    Stopped,
}

fn read_header(stream: &mut TcpStream, stopping: &AtomicBool) -> io::Result<Header> {
    let started = Instant::now();
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    loop {
        if stopping.load(Ordering::Acquire) {
            return Ok(Header::Stopped);
        }
        if started.elapsed() >= HEADER_DEADLINE {
            return Ok(Header::TimedOut);
        }
        match stream.read(&mut buffer) {
            Ok(0) if request.is_empty() => return Ok(Header::Closed),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete request",
                ));
            }
            Ok(count) => {
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    if end + 4 > MAX_HEADER_BYTES {
                        return Ok(Header::TooLarge);
                    }
                    request.truncate(end + 4);
                    return Ok(Header::Complete(request));
                }
                if request.len() >= MAX_HEADER_BYTES {
                    return Ok(Header::TooLarge);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

fn serve(
    stream: &mut TcpStream,
    path: &str,
    snapshot: &RwLock<Arc<Snapshot>>,
    stopping: &AtomicBool,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_SLICE))?;
    stream.set_write_timeout(Some(IO_SLICE))?;
    let header = match read_header(stream, stopping) {
        Ok(Header::Complete(header)) => header,
        Ok(Header::Closed | Header::Stopped) => return Ok(()),
        Ok(Header::TooLarge) => {
            return respond(
                stream,
                431,
                "Request Header Fields Too Large",
                "text/plain; charset=utf-8",
                b"Request header too large",
                false,
                stopping,
            );
        }
        Ok(Header::TimedOut) => {
            return respond(
                stream,
                408,
                "Request Timeout",
                "text/plain; charset=utf-8",
                b"Request timed out",
                false,
                stopping,
            );
        }
        Err(_) => {
            return respond(
                stream,
                400,
                "Bad Request",
                "text/plain; charset=utf-8",
                b"Invalid request",
                false,
                stopping,
            );
        }
    };
    let Some(line) = std::str::from_utf8(&header)
        .ok()
        .and_then(|header| header.lines().next())
    else {
        return respond(
            stream,
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"Invalid request",
            false,
            stopping,
        );
    };
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 || !matches!(parts[2], "HTTP/1.0" | "HTTP/1.1") {
        return respond(
            stream,
            400,
            "Bad Request",
            "text/plain; charset=utf-8",
            b"Invalid request",
            false,
            stopping,
        );
    }
    let head = parts[0] == "HEAD";
    if !matches!(parts[0], "GET" | "HEAD") {
        return respond(
            stream,
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            b"Only GET and HEAD are supported",
            false,
            stopping,
        );
    }
    let target_path = parts[1].split('?').next().unwrap_or("");
    if target_path != path {
        return respond(
            stream,
            404,
            "Not Found",
            "text/plain; charset=utf-8",
            b"Preview page not found",
            head,
            stopping,
        );
    }
    if stopping.load(Ordering::Acquire) {
        return Ok(());
    }
    let snapshot = snapshot
        .read()
        .map_err(|_| io::Error::other("preview snapshot unavailable"))?
        .clone();
    // The shared snapshot RwLock has already been released. Native compilation
    // can be expensive; it belongs to this immutable snapshot's cache only.
    let html = snapshot.native_preview().and_then(|native| {
        export::html_with_native(&snapshot.project, &snapshot.page_keys, native)
    });
    match html {
        Ok(html) => respond(
            stream,
            200,
            "OK",
            "text/html; charset=utf-8",
            html.as_bytes(),
            head,
            stopping,
        ),
        Err(error) => respond(
            stream,
            500,
            "Internal Server Error",
            "text/plain; charset=utf-8",
            format!("无法生成预览：{error:#}。请检查工程后刷新。编辑内容未被修改。").as_bytes(),
            head,
            stopping,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
    head: bool,
    stopping: &AtomicBool,
) -> io::Result<()> {
    let allow = if status == 405 {
        "Allow: GET, HEAD\r\n"
    } else {
        ""
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'\r\n{allow}\r\n",
        body.len(),
    );
    let started = Instant::now();
    bounded_write(stream, header.as_bytes(), stopping, started)?;
    if !head {
        bounded_write(stream, body, stopping, started)?;
    }
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

fn bounded_write(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    stopping: &AtomicBool,
    started: Instant,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        if started.elapsed() >= WRITE_DEADLINE {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "preview response timed out",
            ));
        }
        match stream.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "preview response closed",
                ));
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Response {
        status: u16,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    fn request(server: &PreviewServer, method: &str, path: &str, extra: &str) -> Response {
        let mut stream = TcpStream::connect(server.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "{method} {path} HTTP/1.1\r\nHost: {}\r\n{extra}\r\n",
                    server.address
                )
                .as_bytes(),
            )
            .unwrap();
        let mut bytes = Vec::new();
        match stream.read_to_end(&mut bytes) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset && !bytes.is_empty() => {}
            Err(error) => panic!("HTTP response failed: {error}"),
        }
        let end = bytes
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let text = std::str::from_utf8(&bytes[..end]).unwrap();
        let mut lines = text.lines();
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        Response {
            status,
            headers,
            body: bytes[end + 4..].to_vec(),
        }
    }

    #[test]
    fn serves_utf8_latest_snapshot_and_head_has_the_same_byte_length() {
        let mut project = Project::blank();
        project.name = "最初的本地预览".into();
        let mut server = PreviewServer::start(project.clone()).unwrap();
        assert!(server.address.ip().is_loopback());
        assert!(server.url(1).ends_with("/#page-1"));
        let first = request(&server, "GET", &server.path, "");
        assert_eq!(first.status, 200);
        assert_eq!(first.headers["content-type"], "text/html; charset=utf-8");
        assert_eq!(first.headers["cache-control"], "no-store");
        assert_eq!(
            first.headers["content-length"].parse::<usize>().unwrap(),
            first.body.len()
        );
        assert!(
            std::str::from_utf8(&first.body)
                .unwrap()
                .contains("最初的本地预览")
        );
        let head = request(&server, "HEAD", &server.path, "");
        assert_eq!(head.status, 200);
        assert_eq!(
            head.headers["content-length"],
            first.headers["content-length"]
        );
        assert!(head.body.is_empty());
        project.name = "刷新后的工程".into();
        server.publish(project).unwrap();
        let next = request(&server, "GET", &server.path, "");
        let text = std::str::from_utf8(&next.body).unwrap();
        assert!(text.contains("刷新后的工程"));
        assert!(!text.contains("最初的本地预览"));
    }

    #[test]
    fn rejects_unknown_paths_methods_and_oversized_headers() {
        let server = PreviewServer::start(Project::blank()).unwrap();
        for path in [
            "/",
            "/Cargo.toml",
            "/../Cargo.toml",
            &format!("{}not-a-file", server.path),
        ] {
            assert_eq!(request(&server, "GET", path, "").status, 404);
        }
        let missing = request(&server, "HEAD", "/", "");
        assert_eq!(missing.status, 404);
        assert!(missing.body.is_empty());
        let method = request(&server, "POST", &server.path, "");
        assert_eq!(method.status, 405);
        assert_eq!(method.headers["allow"], "GET, HEAD");
        let large = format!("X-Large: {}\r\n", "a".repeat(MAX_HEADER_BYTES));
        assert_eq!(request(&server, "GET", &server.path, &large).status, 431);
    }

    #[test]
    fn invalid_publish_keeps_previous_snapshot_and_each_server_is_isolated() {
        let mut server = PreviewServer::start(Project::blank()).unwrap();
        let second = PreviewServer::start(Project::blank()).unwrap();
        assert_ne!(server.path, second.path);
        let previous = request(&server, "GET", &server.path, "").body;
        let mut invalid = Project::blank();
        invalid.pages.clear();
        assert!(server.publish(invalid).is_err());
        assert_eq!(request(&server, "GET", &server.path, "").body, previous);
        assert_eq!(request(&server, "GET", &second.path, "").status, 404);
    }

    #[test]
    fn drop_closes_listener_even_with_an_incomplete_browser_request() {
        let server = PreviewServer::start(Project::blank()).unwrap();
        let address = server.address;
        let mut stalled = TcpStream::connect(address).unwrap();
        stalled
            .write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n", server.path).as_bytes())
            .unwrap();
        thread::sleep(Duration::from_millis(30));
        let started = Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(TcpStream::connect_timeout(&address, IO_SLICE).is_err());
    }

    fn player_state(response: &Response) -> serde_json::Value {
        let html = std::str::from_utf8(&response.body).unwrap();
        let json = html
            .split("id=\"prototype-data\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap();
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn logical_page_key_survives_numeric_reassignment_in_url_and_latest_html() {
        let mut project = Project::blank();
        let logical = "包 /?&=#%+~".to_owned();
        let mut server = PreviewServer::start_with_page_keys(
            project.clone(),
            BTreeMap::from([(1, logical.clone())]),
        )
        .unwrap();
        let first_url = server.url(1);
        assert!(first_url.ends_with("#page-1&key=%E5%8C%85%20%2F%3F%26%3D%23%25%2B~"));
        let first = player_state(&request(&server, "GET", &server.path, ""));
        assert_eq!(first["pages"][0]["id"], "1");
        assert_eq!(first["pages"][0]["key"], logical);
        project.pages[0].id = 41;
        project.next_id = 42;
        server
            .publish_with_page_keys(project, BTreeMap::from([(41, logical.clone())]))
            .unwrap();
        let next_url = server.url(41);
        assert_eq!(
            first_url.split("&key=").nth(1),
            next_url.split("&key=").nth(1)
        );
        assert!(next_url.contains("#page-41&key="));
        // The browser's old fragment is retained locally; HTTP receives only
        // this same session path and returns a coherent new ID/key snapshot.
        let refreshed = player_state(&request(&server, "GET", &server.path, ""));
        assert_eq!(refreshed["pages"][0]["id"], "41");
        assert_eq!(refreshed["pages"][0]["key"], logical);
        assert_eq!(encode_fragment_value("AZaz09-._~"), "AZaz09-._~");
    }

    #[test]
    fn invalid_page_keys_never_replace_document_or_mapping_snapshot() {
        let project = Project::default();
        let first = project.pages[0].id;
        let second = project.pages[1].id;
        let mut server = PreviewServer::start_with_page_keys(
            project.clone(),
            BTreeMap::from([(first, "stable-first".into())]),
        )
        .unwrap();
        let before = request(&server, "GET", &server.path, "").body;
        let before_url = server.url(first);
        let mut edited = project.clone();
        edited.name = "should never be published".into();
        for keys in [
            BTreeMap::from([(u64::MAX, "unknown-page".into())]),
            BTreeMap::from([(first, "duplicate".into()), (second, "duplicate".into())]),
            BTreeMap::from([(first, format!("id-{second}"))]),
            BTreeMap::from([(first, String::new())]),
            BTreeMap::from([(first, "x".repeat(1025))]),
        ] {
            assert!(PreviewServer::start_with_page_keys(project.clone(), keys.clone()).is_err());
            assert!(server.publish_with_page_keys(edited.clone(), keys).is_err());
            assert_eq!(request(&server, "GET", &server.path, "").body, before);
            assert_eq!(server.url(first), before_url);
        }
    }

    #[test]
    #[ignore = "requires a caller-selected native fixture; source is read-only"]
    fn lazy_native_http_snapshots_cache_once_and_preserve_unsaved_edits_and_source() {
        let Some(path) = std::env::var_os("RPSHOP_NATIVE_FIXTURE") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let original = std::fs::read(&path).unwrap();
        let document = crate::rp_format::open(&path).unwrap();
        let baseline = document.project.clone();
        let session = document.session_state();
        let started = Instant::now();
        let source = Arc::new(document.clone());
        eprintln!(
            "native document snapshot clone: {}ms",
            started.elapsed().as_millis()
        );
        let mut draft = baseline.clone();
        let (page_index, widget_index) = draft
            .pages
            .iter()
            .enumerate()
            .find_map(|(pi, page)| {
                page.widgets.iter().enumerate().find_map(|(wi, widget)| {
                    (document.is_editable_widget(widget.id) && !widget.text.is_empty())
                        .then_some((pi, wi))
                })
            })
            .expect("fixture needs editable text");
        draft.pages[page_index].widgets[widget_index].text =
            "Lazy snapshot first unsaved text".into();
        let keys = document.preview_page_keys();
        let mut server = PreviewServer::start_with_native_document(
            draft.clone(),
            keys.clone(),
            Some(Arc::clone(&source)),
        )
        .unwrap();
        let first_snapshot = Arc::clone(&server.snapshot.read().unwrap());
        let NativeSnapshot::Document {
            compiled: first_cache,
            ..
        } = &first_snapshot.native
        else {
            panic!("expected lazy source");
        };
        assert!(
            first_cache.get().is_none(),
            "publishing must not compile on the editor thread"
        );
        let first = request(&server, "GET", &server.path, "");
        assert_eq!(first.status, 200);
        assert!(
            player_state(&first)["native"]["scene"]
                .to_string()
                .contains("Lazy snapshot first unsaved text")
        );
        let cached = Arc::clone(first_cache.get().unwrap().as_ref().unwrap());
        let head = request(&server, "HEAD", &server.path, "");
        assert_eq!(head.status, 200);
        assert_eq!(
            head.headers["content-length"],
            first.headers["content-length"]
        );
        assert!(head.body.is_empty());
        assert!(Arc::ptr_eq(
            &cached,
            first_cache.get().unwrap().as_ref().unwrap()
        ));

        draft.pages[page_index].widgets[widget_index].text =
            "Lazy snapshot second unsaved text".into();
        server
            .publish_with_native_document(draft.clone(), keys.clone(), Some(Arc::clone(&source)))
            .unwrap();
        let second_snapshot = Arc::clone(&server.snapshot.read().unwrap());
        assert!(!Arc::ptr_eq(&first_snapshot, &second_snapshot));
        let NativeSnapshot::Document {
            compiled: second_cache,
            ..
        } = &second_snapshot.native
        else {
            panic!("expected new lazy source");
        };
        assert!(second_cache.get().is_none());
        let second = request(&server, "GET", &server.path, "");
        assert_eq!(second.status, 200);
        let state = player_state(&second)["native"]["scene"].to_string();
        assert!(state.contains("Lazy snapshot second unsaved text"));
        assert!(!state.contains("Lazy snapshot first unsaved text"));
        assert!(!Arc::ptr_eq(
            &cached,
            second_cache.get().unwrap().as_ref().unwrap()
        ));
        assert!(
            first_snapshot.project.pages[page_index].widgets[widget_index].text
                == "Lazy snapshot first unsaved text"
        );

        // An unsupported source edit fails only when requested. Cache the error
        // too, report it explicitly over HTTP, and keep every editor draft.
        let mut unsupported = draft.clone();
        unsupported.pages[0].canvas =
            if unsupported.pages[0].canvas == crate::model::CanvasMode::Auto {
                crate::model::CanvasMode::Fixed
            } else {
                crate::model::CanvasMode::Auto
            };
        server
            .publish_with_native_document(unsupported, keys.clone(), Some(Arc::clone(&source)))
            .unwrap();
        let failed_snapshot = Arc::clone(&server.snapshot.read().unwrap());
        let NativeSnapshot::Document {
            compiled: failed_cache,
            ..
        } = &failed_snapshot.native
        else {
            panic!("expected lazy source");
        };
        assert!(failed_cache.get().is_none());
        let failed = request(&server, "GET", &server.path, "");
        assert_eq!(failed.status, 500);
        assert!(
            std::str::from_utf8(&failed.body)
                .unwrap()
                .contains("原生预览构建失败")
        );
        assert!(failed_cache.get().unwrap().is_err());
        assert_eq!(request(&server, "GET", &server.path, "").body, failed.body);
        server
            .publish_with_native_document(draft.clone(), keys, Some(Arc::clone(&source)))
            .unwrap();
        let recovered = request(&server, "GET", &server.path, "");
        assert_eq!(recovered.status, 200);
        assert!(
            player_state(&recovered)["native"]["scene"]
                .to_string()
                .contains("Lazy snapshot second unsaved text")
        );
        assert!(document.project == baseline && source.project == baseline);
        assert!(document.session_state() == session && source.session_state() == session);
        assert!(
            draft.pages[page_index].widgets[widget_index].text
                == "Lazy snapshot second unsaved text"
        );
        assert!(std::fs::read(&path).unwrap() == original);
    }
}
