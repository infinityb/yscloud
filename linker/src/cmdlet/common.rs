use clap::Arg;

pub fn registry() -> Arg<'static, 'static> {
    Arg::with_name("registry")
        .long("registry")
        .value_name("DIR")
        .help("an artifact registry directory containing metadata about the available artifacts")
        .required(true)
        .takes_value(true)
        .validator_os(|_| Ok(()))
}

pub fn approot() -> Arg<'static, 'static> {
    Arg::with_name("approot")
        .long("approot")
        .value_name("DIR")
        .help("an application state directory root")
        .required(true)
        .takes_value(true)
        .validator_os(|_| Ok(()))
}

pub fn artifacts() -> Arg<'static, 'static> {
    Arg::with_name("artifacts")
        .long("artifacts")
        .value_name("DIR-or-URL")
        .help("an artifact directory containing dependencies of the manifest")
        .required(true)
        .takes_value(true)
}

pub fn artifact_override() -> Arg<'static, 'static> {
    Arg::with_name("artifact-override")
        .long("artifact-override")
        .value_name("PACKAGE_ID:PATH")
        .help("Override a Package ID with some other path")
        .multiple(true)
        .takes_value(true)
}
