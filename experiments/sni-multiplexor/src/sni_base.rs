use failure::Error;
use tls::{ClientHello, Extension, ExtensionServerName};

use crate::error::tls::ALERT_INTERNAL_ERROR;

pub fn get_server_names<'arena>(
    hello: &ClientHello<'arena>,
) -> Result<&'arena ExtensionServerName<'arena>, Error> {
    for ext in hello.extensions.0 {
        match *ext {
            Extension::ServerName(name_ext) => return Ok(name_ext),
            Extension::Unknown(..) => (),
        }
    }
    Err(ALERT_INTERNAL_ERROR.into())
}
