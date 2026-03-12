use agent_runtime::audit::verify_chain;

fn usage() -> String {
    "Usage:\n  auditctl verify --path <audit.jsonl>".into()
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find_map(|w| (w[0] == name).then(|| w[1].clone()))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("{}", usage());
        std::process::exit(2);
    }

    match args[1].as_str() {
        "verify" => {
            let Some(path) = parse_flag(&args[2..], "--path") else {
                eprintln!("missing --path\n{}", usage());
                std::process::exit(2);
            };

            match verify_chain(&path) {
                Ok(()) => println!("audit chain verification PASSED"),
                Err(e) => {
                    eprintln!("audit chain verification FAILED: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("unknown command\n{}", usage());
            std::process::exit(2);
        }
    }
}
