use std::io;

use sockets::{Listener, Connected};
use yscloud_config_model::{AppConfiguration, ServiceFileDirection};


pub fn get_listening_socket(cfg: &AppConfiguration, name: &str) -> io::Result<Listener> {
    for file in &cfg.files {
        if file.direction == ServiceFileDirection::ServingListening && file.service_name == name {
            // need to find a way to disallow duplication of this item
            return Ok(unsafe { Listener::from_raw_fd(file.file_num) });
        }
    }

    get_service(cfg, name)
}

pub fn get_connected_socket(cfg: &AppConfiguration, name: &str) -> io::Result<Connected> {
for file in &cfg.files {
        if file.direction == ServiceFileDirection::ServingConnected && file.service_name == name {
            // need to find a way to disallow duplication of this item
            return Ok(unsafe { Connected::from_raw_fd(file.file_num) });
        }
    }
    unimplemented!("put in a not-found error here");
}

pub fn get_service(cfg: &AppConfiguration, name: &str) -> io::Result<Listener> {
    let mut connections = Vec::new();
    for file in &cfg.files {
        if file.direction == ServiceFileDirection::ServingConnected && file.service_name == name {
            // need to find a way to disallow duplication of this item
            connections.push(unsafe { Connected::from_raw_fd(file.file_num) });
        }
    }
    if !connections.is_empty() {
        return Ok(Listener::fixed(connections))
    }
    unimplemented!("put in a not-found erorr here");

}