use std::{
    env, fs,
    io::{self, Read, Write},
    process::Command,
    thread,
    time::Duration,
};
fn main() {
    match env::args().nth(1).as_deref() {
        Some("input") => {
            let mut bytes = Vec::new();
            io::stdin().read_to_end(&mut bytes).unwrap();
            assert!(bytes.is_empty());
            println!("stdin EOF");
        }
        Some("output") => {
            let bytes = vec![b'x'; 2 * 1024 * 1024];
            io::stdout().write_all(&bytes).unwrap();
            io::stderr().write_all(&bytes).unwrap();
        }
        Some(mode @ ("tree" | "tree-exit")) => {
            let child = Command::new(env::current_exe().unwrap())
                .arg("leaf")
                .spawn()
                .unwrap();
            fs::write(
                env::args_os().nth(2).unwrap(),
                format!("{} {}", std::process::id(), child.id()),
            )
            .unwrap();
            if mode == "tree-exit" {
                std::process::exit(23);
            }
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Some("leaf") => loop {
            thread::sleep(Duration::from_secs(1));
        },
        _ => panic!("unknown fixture mode"),
    }
}
