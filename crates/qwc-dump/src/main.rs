//! `qwc-dump [--sfa STREAM.sfa] FILE...`: print one JSON line per file (schema shared with `tools/dump_json.py`).
//! A file that fails to parse prints `{"error": "..."}` and makes the exit status nonzero.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut sfa = None;
    let mut files = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--sfa" => sfa = args.next(),
            "-h" | "--help" => {
                eprintln!(
                    "usage: qwc-dump [--sfa STREAM.sfa] FILE...  (.ccd .fsh .skl .fpm .fas .sfa .btf .sts .sdx .ffn .qsp \
                     .vso, cutscene .txt, \
                     .pot .lot .trl .got .cli .gro, .fat .mus .hdr, Audio dat.dat and banks.dat)"
                );
                return ExitCode::SUCCESS;
            }
            _ => files.push(a),
        }
    }
    let mut status = ExitCode::SUCCESS;
    for f in &files {
        let v = qwc_formats::dump::dump_path(f, sfa.as_deref()).unwrap_or_else(|e| {
            status = ExitCode::FAILURE;
            serde_json::json!({ "error": e.to_string() })
        });
        println!("{v}");
    }
    status
}
