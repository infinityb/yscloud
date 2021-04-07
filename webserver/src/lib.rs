use std::fmt;
use std::time::Duration;

use askama::Template;
use uuid::Uuid;
use ksuid::Ksuid;

const STATUS_FORBIDDEN: u16 = 403;
const STATUS_NOT_FOUND: u16 = 404;

fn default_status_code_text(status_code: u16) -> &'static str {
    match status_code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "(Unknown Status)",
    }
}

fn default_error_title(status_code: u16) -> &'static str {
    match status_code {
        STATUS_FORBIDDEN => return "You don't have access to this resource.",
        STATUS_NOT_FOUND => return "The resource you were trying to reach does not exist.",
        x if 400 <= x && x < 500 => "An unknown client error occurred",
        x if 500 <= x && x < 600 => "Sorry, we can't service your request right now.",
        _ => "An unknown error occurred",
    }
}

#[derive(Template)]
#[template(path = "errorpage.html")]
pub struct ErrorPageTemplate<'a> {
    pub status_code: u16,
    pub status_code_text: &'static str,
    pub error_title: &'a str,
    pub error_string: &'a str,
    pub request_id: Ksuid,
    pub fe_instance_id: Uuid,
}

impl<'a> ErrorPageTemplate<'a> {
    fn status_code_text(&self) -> &'a str {
        if !self.status_code_text.is_empty() {
            return self.status_code_text
        }
        default_status_code_text(self.status_code)
    }

    fn error_title(&self) -> &'a str {
        if !self.error_title.is_empty() {
            return self.error_title
        }
        default_error_title(self.status_code)
    }
}

#[derive(Template)]
#[template(path = "directory_listing.html")]
pub struct DirectoryListingTemplate<'a> {
    pub current_path: &'a str,
    pub is_root: bool,
    pub entries: Vec<DirectoryListingEntry<'a>>,
    pub render_time: Duration,
    pub request_id: Ksuid,
    pub fe_instance_id: Uuid,
}

pub struct DirectoryListingEntry<'a> {
    pub is_directory: bool,
    pub file_type: &'a str,
    pub file_name: &'a str,
    pub file_size_bytes: u64,
}

impl DirectoryListingEntry<'_> {
    fn directory_trailer(&self) -> &'static str {
        if self.is_directory { "/" } else { "" }
    }

    fn file_size_human_readable(&self) -> ByteSize {
        ByteSize(self.file_size_bytes)
    }
}

#[test]
fn main_render_404() {
    return;
    let hello = ErrorPageTemplate {
        status_code: 404,
        status_code_text: "",
        error_title: "",
        error_string: "",
        request_id: Ksuid::generate(),
        fe_instance_id: Uuid::new_v4(),
    };
    println!("{}", hello.render().unwrap());
}


struct ByteSize(u64);

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        const UNIT_NAMES: &[&str] = &["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];
        let mut unit_acc = self.0;
        let mut unit_num = 0;
        while 1024 <= unit_acc && unit_num < UNIT_NAMES.len() {
            unit_acc /= 1024;
            unit_num += 1;
        }

        let value = match unit_num {
            0 => self.0 as f64,
            1 => self.0 as f64 / 1024.0,
            _ => {
                // use integer division first, to ensure that the value
                // fits into floats
                let unit_denom_int = 1 << (10 * (unit_num - 1));
                (self.0 / unit_denom_int) as f64 / 1024.0
            }
        };

        if unit_num == 0 {
            write!(f, "{}B", value)
        } else {
            write!(f, "{:.1}{}B", value, UNIT_NAMES[unit_num])
        }
    }
}
