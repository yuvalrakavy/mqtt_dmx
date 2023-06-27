
use log::info;
use thiserror::Error;
use tokio::{select, sync::mpsc::Receiver};
use tokio_util::sync::CancellationToken;
use std::collections::HashMap;

use crate::messages::ToArrayManagerMessage;
use crate::defs::DmxArray;


#[derive(Debug, Error)]
pub enum DmxArrayError {

}

pub struct ArrayManager {
    arrays: HashMap<String, DmxArray>,
}

impl ArrayManager {
    pub fn new() -> Self {
        Self {
            arrays: HashMap::new(),
        }
    }

    pub fn add_array(&mut self, name: String, array: DmxArray) -> Result<(), DmxArrayError> {
        self.arrays.insert(name, array);
        Ok(())
    }

    pub fn remove_array(&mut self, name: String) -> Result<(), DmxArrayError> {
        self.arrays.remove(&name);
        Ok(())
    }

    fn handle_message(&mut self, message: ToArrayManagerMessage) {
         match message {
            ToArrayManagerMessage::AddArray(name, array, reply_tx) =>
                reply_tx.send(self.add_array(name, array)).unwrap(),

            ToArrayManagerMessage::RemoveArray(name, reply_tx) =>
                reply_tx.send(self.remove_array(name)).unwrap(),
        }
    }

    pub async fn run(&mut self, cancel: CancellationToken, mut receiver: Receiver<ToArrayManagerMessage>) {
        loop {
            select! {
                _ = cancel.cancelled() => break,

                message = receiver.recv() => match message {
                    None => break,
                    Some(message) => self.handle_message(message),
                },
            }
        }

        info!("ArtnetManager stopped");
    }

}