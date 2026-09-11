//! Test-only SSH-shaped process. Compiled by native_process.rs into a temp dir.
use std::{env,fs,io::Write,net::TcpListener,path::{Path,PathBuf},process::Command,thread,time::Duration};
fn record(directory:&Path){
    let mut file=fs::OpenOptions::new().create(true).append(true).open(directory.join("fixture-pids")).unwrap();
    writeln!(file,"{}",std::process::id()).unwrap();
}
fn main(){
    let args=env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg|arg=="--descendant"){
        fs::write(&args[2],std::process::id().to_string()).unwrap();
        let directory=Path::new(&args[2]).parent().unwrap();record(directory);
        // Parent death intentionally does NOT stop this proxy. The explicit
        // test-only marker is a failure-cleanup fallback, never a pass oracle.
        while !directory.join("fixture-stop").exists(){thread::sleep(Duration::from_millis(100));}
        return;
    }
    let config=PathBuf::from(&args[args.iter().position(|arg|arg=="-F").unwrap()+1]);
    let directory=if config.is_file(){config.parent().unwrap().to_path_buf()}else{config};
    if directory.join("fixture-stop").exists(){return;}
    let forward=&args[args.iter().position(|arg|arg=="-L").unwrap()+1];
    let port=forward.split(':').nth(1).unwrap().parse::<u16>().unwrap();
    fs::write(directory.join("parent.pid"),std::process::id().to_string()).unwrap();
    record(&directory);
    let listener=TcpListener::bind(("127.0.0.1",port)).unwrap();listener.set_nonblocking(true).unwrap();
    let _descendant=Command::new(env::current_exe().unwrap()).arg("--descendant").arg(directory.join("child.pid")).spawn().unwrap();
    loop{
        if directory.join("fixture-stop").exists(){return;}
        if directory.join("exit").exists(){eprintln!("Connection reset by peer");std::process::exit(255);}
        if let Ok((mut connection,_))=listener.accept(){let _=connection.write_all(b"fixture-ok");}
        thread::sleep(Duration::from_millis(10));
    }
}
