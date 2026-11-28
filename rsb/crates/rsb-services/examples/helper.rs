//! A harmless program the exec tests run: it prints, sleeps, exits and
//! starts a child of its own, nothing else.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    let number = |i: usize| arg(i).parse::<u64>().unwrap_or(0);
    let out = std::io::stdout();
    match arg(0) {
        // Each argument on a line in brackets, so quoting can be checked exactly.
        "args" => {
            for a in &args[1..] {
                println!("[{a}]");
            }
        }
        "sleep" => std::thread::sleep(Duration::from_millis(number(1))),
        // `lines <count> <delay ms>`
        "lines" => {
            for i in 0..number(1) {
                println!("line {i}");
                let _ = out.lock().flush();
                std::thread::sleep(Duration::from_millis(number(2)));
            }
        }
        "stderr" => {
            eprintln!("to stderr");
            println!("to stdout");
        }
        "exit" => std::process::exit(number(1) as i32),
        // `flood <bytes>`: that many bytes of text on stdout.
        "flood" => {
            let mut left = number(1) as usize;
            let line = format!("{}\n", "x".repeat(99));
            let mut lock = out.lock();
            while left > 0 {
                let n = left.min(line.len());
                if lock.write_all(&line.as_bytes()[..n]).is_err() {
                    break;
                }
                left -= n;
            }
        }
        // `longline <bytes>`: one line without a break, then a short one.
        "longline" => {
            let mut lock = out.lock();
            let _ = lock.write_all("y".repeat(number(1) as usize).as_bytes());
            let _ = lock.write_all(b"\nafter\n");
        }
        // Starts a child that outlives it unless the tree is killed, prints the child's id, then waits.
        "spawn-child" => {
            let me = std::env::current_exe().expect("own path");
            let child = Command::new(me)
                .args(["sleep", "60000"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("child starts");
            println!("{}", child.id());
            let _ = out.lock().flush();
            std::thread::sleep(Duration::from_millis(number(1).max(1)));
            // Reached only when nobody killed the tree first.
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
        }
        // A child that holds our output pipe open after we exit.
        "leak-pipe" => {
            let me = std::env::current_exe().expect("own path");
            let child = Command::new(me).args(["sleep", "60000"]).stdin(Stdio::null()).spawn().expect("child starts");
            println!("{}", child.id());
            // Deliberately left running: the test checks that the tree is killed. Not waited for.
            std::mem::forget(child);
        }
        "cwd-name" => {
            let cwd = std::env::current_dir().expect("cwd");
            println!("{}", cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
        "stdin-eof" => {
            let mut s = String::new();
            let n = std::io::stdin().read_line(&mut s).unwrap_or(99);
            println!("stdin read {n}");
        }
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}
