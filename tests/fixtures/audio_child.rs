// Synthetic process fixture only. No microphone, speakers, network or user keys.
use std::{env, fs, io::{Read, Write}, path::PathBuf, thread, time::Duration};
fn main() {
    let root = PathBuf::from(env::var_os("PORTS_AUDIO_FIXTURE").unwrap());
    let name = env::current_exe().unwrap().file_stem().unwrap().to_string_lossy().to_lowercase();
    fs::write(root.join(format!("{name}.pid")), std::process::id().to_string()).unwrap();
    if name == "ffmpeg" {
        let data: Vec<u8> = (0..2560).map(|n| (n % 256) as u8).collect();
        loop {
            if std::io::stdout().write_all(&data).is_err() { break; }
            thread::sleep(Duration::from_millis(10));
        }
    } else if name == "ffplay" {
        if env::var_os("AUDIO_EARLY_EXIT").is_some() { thread::sleep(Duration::from_millis(700)); std::process::exit(7); }
        let mut total = 0;
        let mut invalid = 0;
        let mut bytes = [0u8; 4096];
        loop {
            let n = std::io::stdin().read(&mut bytes).unwrap_or(0);
            if n == 0 { break; }
            for byte in &bytes[..n] { if *byte != (total % 256) as u8 { invalid += 1; } total += 1; }
            fs::write(root.join("received.txt"),format!("{total} {invalid}")).unwrap();
        }
    } else if name == "ssh" {
        let command = env::args().last().unwrap();
        if command.ends_with("duplex") {
            fs::write(root.join("active"),"yes").unwrap();
            eprintln!("SSH voice ready: synthetic fixture");
            let mut bytes = [0u8; 4096];
            loop {
                let n = std::io::stdin().read(&mut bytes).unwrap_or(0);
                if n == 0 || std::io::stdout().write_all(&bytes[..n]).is_err() { break; }
            }
            let _ = fs::remove_file(root.join("active"));
        } else {
            if command.ends_with("deactivate") { let _ = fs::remove_file(root.join("active")); }
            if env::var_os("AUDIO_SLOW_STATUS").is_some() { thread::sleep(Duration::from_millis(500)); }
            let active = root.join("active").exists();
            let (source,sink) = if active { ("codex_ssh_mic","codex_ssh_reply") } else { ("original-source","original-sink") };
            println!("{{\"host\":\"fixture\",\"defaults\":{{\"source\":\"{source}\",\"sink\":\"{sink}\"}},\"state\":{{\"active\":{active}}}}}");
        }
    }
}
