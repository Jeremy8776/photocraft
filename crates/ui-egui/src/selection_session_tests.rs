//! Scripted local HTTP evidence, not real model inference.
use super::*;
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

type RecordedRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct Server {
    url: String,
    requests: RecordedRequests,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(mask: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (recorded, stopping) = (Arc::clone(&requests), Arc::clone(&stop));
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                // Windows accepts the listener's non-blocking mode; request reads must wait.
                stream.set_nonblocking(false).unwrap();
                let (path, body) = request(&mut stream);
                recorded.lock().unwrap().push((path.clone(), body));
                let response = match path.as_str() {
                    "/system_stats" => json!({"system":{"comfyui_version":"test"}}),
                    "/object_info" => {
                        json!({"LoadImage":{},"Florence2ModelLoader":{},"Florence2Run":{},"Florence2toCoordinates":{},"DownloadAndLoadSAM2Model":{},"Sam2Segmentation":{},"MaskToImage":{},"SaveImage":{}})
                    }
                    "/upload/image" => json!({"name":"source.png","type":"input","subfolder":""}),
                    "/prompt" => json!({"prompt_id":"sam21-app"}),
                    "/history/sam21-app" => {
                        json!({"sam21-app":{"status":{"completed":true},"outputs":{"8":{"images":[{"filename":"mask.png","type":"output","subfolder":""}]}}}})
                    }
                    _ => json!({}),
                };
                let bytes = if path.starts_with("/view?") { mask.clone() } else { response.to_string().into_bytes() };
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&bytes).unwrap();
            }
        });
        Self { url, requests, stop, worker: Some(worker) }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let end = loop {
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0 && bytes.len() < 64 * 1024);
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&bytes[..end]);
    let path = head.split_whitespace().nth(1).unwrap().to_owned();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length < 16 * 1024 * 1024);
    while bytes.len() - end < length {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
    }
    (path, bytes[end..].to_vec())
}

#[test]
fn accepted_mock_socket_waits_for_delayed_headers() {
    let server = Server::new(Vec::new());
    let mut stream = TcpStream::connect(server.url.trim_start_matches("http://")).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let written = stream.write_all(b"GET /system_stats HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    let mut response = Vec::new();
    let read = stream.read_to_end(&mut response);
    drop(stream);
    drop(server);
    assert!(written.is_ok() && read.is_ok() && response.starts_with(b"HTTP/1.1 200"), "accepted sockets must wait for the request bytes");
}

#[test]
fn selected_sam21_executes_its_own_graph_and_never_sam3() {
    let image = Image::from_u8(8, 8, ChannelLayout::Gray, vec![255; 64]).unwrap();
    let png = photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap();
    let server = Server::new(png.clone());
    let route = SelectionRoute { url: server.url.clone(), model: SelectionModel::Sam21 };
    let mut selected = ModelSession::new(&route).unwrap();
    assert_eq!(selected.model(), SelectionModel::Sam21, "a selected SAM 2.1 must not construct SAM 3");
    selected.prepare(&png).unwrap();
    let mask = photocraft_codecs::decode_as(Format::Png, &selected.ground("person").unwrap()).unwrap();
    assert_eq!(mask.dimensions(), (8, 8));
    assert_eq!(mask.as_u8().unwrap(), &[255; 64]);
    let calls = server.requests.lock().unwrap();
    let graphs: Vec<Value> =
        calls.iter().filter(|(path, _)| path == "/prompt").map(|(_, body)| serde_json::from_slice::<Value>(body).unwrap()["prompt"].clone()).collect();
    assert_eq!(graphs.len(), 1);
    assert_eq!(graphs[0]["5"]["inputs"]["model"], "sam2.1_hiera_large.safetensors");
    assert_eq!(graphs[0]["3"]["inputs"]["text_input"], "person");
    assert!(!calls.iter().any(|(path, _)| path.starts_with("/sam3/")));
    assert!(!graphs.iter().any(|g| g.to_string().contains("LoadSAM3Model")));
}
