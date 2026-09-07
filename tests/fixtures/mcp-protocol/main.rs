// 仅合成测试程序：不链接主 CLI、不读用户数据。
use wx_mcp_protocol_harness::{
    ipc::{Request, Response},
    protocol::{Protocol, DEFAULT_MAX_FRAME_BYTES},
};

fn main() {
    let mut protocol = Protocol::new(|request| {
        if matches!(&request, Request::Contacts { query: Some(query), .. } if query == "synthetic-error")
        {
            return Ok(Response::err(
                "private-message=SYNTHETIC_PRIVATE keys=SYNTHETIC_SECRET",
            ));
        }
        Ok(Response::ok(serde_json::json!({
            "synthetic": true, "request": request, "text": "first\nsecond"
        })))
    });
    if let Err(error) = protocol.serve(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        DEFAULT_MAX_FRAME_BYTES,
    ) {
        eprintln!("synthetic transport: {error}");
        std::process::exit(1);
    }
}
