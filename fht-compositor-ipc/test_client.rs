use std::io::{Read, Write};

fn main() {
    let given_path = std::env::args().skip(1).next().unwrap();
    unsafe {
        std::env::set_var("FHTC_SOCKET_PATH", given_path);
    }

    let (_, mut stream) = fht_compositor_ipc::connect().unwrap();
    stream.set_nonblocking(false).unwrap();

    let mut req = serde_json::to_string(&fht_compositor_ipc::Request::Space).unwrap();
    req.push('\n'); // it is required to append a newline.
    let size = stream.write(req.as_bytes()).unwrap();
    assert_eq!(req.len(), size);

    let mut res_buf = String::new();
    let size = stream.read_to_string(&mut res_buf).unwrap();
    assert_eq!(res_buf.len(), size);

    let Result::<_, String>::Ok(fht_compositor_ipc::Response::Space(space)) =
        serde_json::de::from_str(&res_buf).unwrap()
    else {
        panic!()
    };

    let windows_json = serde_json::to_string(&space).unwrap();
    println!("{}", windows_json);
}
