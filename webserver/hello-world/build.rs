fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;
    // use std::process::Command;
    // use std::os::unix::io::AsRawFd;
    // use std::os::unix::io::FromRawFd;

    // let stderr = std::io::stderr();
    // let stderr_stdio = unsafe { std::process::Stdio::from_raw_fd(stderr.as_raw_fd()) };
    // Command::new("env")
    //     .stdout(stderr_stdio)
    //     .spawn()
    //     .expect("env command failed to start");

    // drop(stderr);

    // const CERT_BASE = ;

    // let path = PathBuf::from(env!("CERTIFICATE_ISSUER_PATH"));
    // let mut files = path.clone();
    // files.push("certificate_issuer.proto");

    // eprintln!("path = {}", path.display());
    // eprintln!("files = {}", files.display());

    // tonic_build::configure()
    //     .format(false)
    //     .compile(&[&files], &[&path])?;
    Ok(())
}