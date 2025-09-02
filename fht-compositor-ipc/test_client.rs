use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

use fht_compositor_ipc::{IpcRequest, Request, Response};

fn write_req(stream: &mut UnixStream, req: Request) -> Response {
    let mut req = serde_json::to_string(&req).unwrap();
    req.push('\n'); // it is required to append a newline.
    stream.write_all(req.as_bytes()).unwrap();

    let mut reader = BufReader::new(stream);
    let mut res_buf = String::new();
    let size = reader.read_line(&mut res_buf).unwrap();
    assert_eq!(res_buf.len(), size);

    serde_json::de::from_str(&res_buf).unwrap()
}

fn main() {
    let (_, mut stream) = fht_compositor_ipc::connect().unwrap();
    // Example where we get two responses
    {
        dbg!(write_req(
            &mut stream,
            IpcRequest {
                request: Request::Space,
                subscribe: false
            }
        ));
        dbg!(write_req(
            &mut stream,
            IpcRequest {
                request: Request::LayerShells,
                subscribe: false
            }
        ));
    }
    stream.set_nonblocking(false).unwrap();
}
