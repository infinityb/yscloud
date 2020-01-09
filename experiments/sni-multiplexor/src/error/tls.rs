use failure::Fail;

pub const ALERT_INTERNAL_ERROR: AlertError = AlertError {
    alert_description: 80,
};

pub const ALERT_UNRECOGNIZED_NAME: AlertError = AlertError {
    alert_description: 112,
};

#[derive(Debug, Copy, Clone, Fail)]
#[fail(display = "TLS error")]
pub struct AlertError {
    alert_description: u8,
}
