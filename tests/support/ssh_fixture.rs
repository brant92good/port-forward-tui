//! Test-only SSH-shaped process. Compiled by native_process.rs into a temp dir.
use std::{env,fs,io::Write,net::TcpListener,path::PathBuf,process::Command,thread,time::Duration};
fn main(){
    let args=env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg|arg=="--descendant"){
        fs::write(&args[2],std::process::id().to_string()).unwrap();
        loop{thread::sleep(Duration::from_millis(100));}
    }
    let config=PathBuf::from(&args[args.iter().position(|arg|arg=="-F").unwrap()+1]);
    let directory=if config.is_file(){config.parent().unwrap().to_path_buf()}else{config};
    let forward=&args[args.iter().position(|arg|arg=="-L").unwrap()+1];
    let port=forward.split(':').nth(1).unwrap().parse::<u16>().unwrap();
    let listener=TcpListener::bind(("127.0.0.1",port)).unwrap();listener.set_nonblocking(true).unwrap();
    fs::write(directory.join("parent.pid"),std::process::id().to_string()).unwrap();
    let _descendant=Command::new(env::current_exe().unwrap()).arg("--descendant").arg(directory.join("child.pid")).spawn().unwrap();
    loop{
        if directory.join("exit").exists(){eprintln!("Connection reset by peer");std::process::exit(255);}
        if let Ok((mut connection,_))=listener.accept(){let _=connection.write_all(b"fixture-ok");}
        thread::sleep(Duration::from_millis(10));
    }
}
