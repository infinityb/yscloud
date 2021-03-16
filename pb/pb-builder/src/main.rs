fn main() -> Result<(), Box<dyn std::error::Error>> {
   let out = std::env::var_os("out").unwrap();

   std::thread::sleep_ms(1000);

   eprintln!("PROTOC = {:?}", std::env::var("PROTOC"));

   tonic_build::configure()
        .format(false)
        .out_dir(&out)
        .compile(
        	&["./certificate_issuer.proto"],
        	&["."],
        )?;

   Ok(())
}