mod lang;
mod worker;

use std::io::{self, BufRead, IsTerminal, Write};

fn main() {
    let mut interp = lang::Interp::new();
    let tty = io::stdin().is_terminal();
    if tty {
        println!("seam 0.1 — a scripting language with borrowed batteries");
        println!("try:  use py \"math\" as m   then   m.sqrt(2)   (:q or ctrl-d to quit)");
    }
    let stdin = io::stdin();
    let mut out = io::stdout();
    let mut input = String::new();
    loop {
        if tty {
            print!("» ");
            out.flush().ok();
        }
        input.clear();
        match stdin.lock().read_line(&mut input) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = input.trim();
        if line.is_empty() {
            continue;
        }
        if line == ":q" {
            break;
        }
        match interp.eval_line(line) {
            Ok(Some(shown)) => println!("{shown}"),
            Ok(None) => {}
            Err(e) => eprintln!("✗ {e}"),
        }
    }
}
