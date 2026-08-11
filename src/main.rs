mod interp;
mod lex;
mod parse;
mod worker;

use std::io::{self, BufRead, IsTerminal, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("seam {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help") | Some("-h") => {
            println!("seam {} — a tiny scripting language with borrowed batteries", env!("CARGO_PKG_VERSION"));
            println!();
            println!("usage:");
            println!("  seam                     REPL (:q or ctrl-d to quit)");
            println!("  seam script.seam [args]  run a script; args land in `argv`");
            println!("  seam < script.seam       pipe mode (echoes expression results)");
            println!();
            println!("`use py ...` needs python3 on PATH (prefers ./.venv/bin/python3);");
            println!("`use js ...` needs node (resolves ./node_modules). Neither spawns");
            println!("until a script asks for it.");
            println!();
            println!("guide: https://github.com/tedfiedler/seam/blob/main/docs/GUIDE.md");
            return;
        }
        _ => {}
    }

    let mut interp = interp::Interp::new();

    // file mode: seam script.seam [args...] (expression results are not echoed)
    if args.len() > 1 {
        interp.set_argv(args.get(2..).unwrap_or(&[]));
        let src = match std::fs::read_to_string(&args[1]) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("✗ can't read {}: {e}", args[1]);
                std::process::exit(1);
            }
        };
        if let Err(e) = interp.run(&src, false) {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
        return;
    }

    // REPL: accumulate lines until braces/brackets/parens balance
    interp.set_argv(&[]);
    let tty = io::stdin().is_terminal();
    if tty {
        println!("seam 0.2 — a scripting language with borrowed batteries");
        println!("try:  use py \"math\" as m   then   m.sqrt(2)   (:q or ctrl-d to quit)");
    }
    let stdin = io::stdin();
    let mut out = io::stdout();
    let mut line = String::new();
    let mut buf = String::new();
    loop {
        if tty {
            print!("{}", if buf.is_empty() { "» " } else { "… " });
            out.flush().ok();
        }
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                if !buf.trim().is_empty() {
                    if let Err(e) = interp.run(&buf, true) {
                        eprintln!("✗ {e}");
                    }
                }
                break;
            }
            Ok(_) => {}
        }
        if buf.is_empty() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t == ":q" {
                break;
            }
        }
        buf.push_str(&line);
        match lex::lex(&buf) {
            Err(e) => {
                eprintln!("✗ {e}");
                buf.clear();
                continue;
            }
            Ok(toks) => {
                let depth: i32 = toks
                    .iter()
                    .map(|(t, _)| match t {
                        lex::Tok::LParen | lex::Tok::LBracket | lex::Tok::LBrace => 1,
                        lex::Tok::RParen | lex::Tok::RBracket | lex::Tok::RBrace => -1,
                        _ => 0,
                    })
                    .sum();
                if depth > 0 {
                    continue;
                }
            }
        }
        if let Err(e) = interp.run(&buf, true) {
            eprintln!("✗ {e}");
        }
        buf.clear();
    }
}
