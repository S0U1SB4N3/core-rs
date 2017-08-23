//! The Profile module exports a struct that is responsible for handling and
//! storing the user's data (keychain, boards, etc) in-memory.
//!
//! It only stores data for the keychain, persona (soon deprecated), and boards
//! (so no note data). The reason is that keychain/boards are useful to keep in
//! memory to decrypt notes, but otherwise, notes can just be loaded on the fly
//! from local storage and discarded once sent to the UI.

use ::models::model::Model;
use ::models::keychain::Keychain;
use ::models::space::Space;
use ::models::board::Board;
use ::models::invite::Invite;

pub struct Profile {
    pub keychain: Keychain,
    pub spaces: Vec<Space>,
    pub boards: Vec<Board>,
    pub invites: Vec<Invite>,
}

impl Profile {
    pub fn new() -> Profile {
        Profile {
            keychain: Keychain::new(),
            spaces: Vec::new(),
            boards: Vec::new(),
            invites: Vec::new(),
        }
    }

    /// Wipe the profile from memory
    pub fn wipe(&mut self) {
        self.keychain = Keychain::new();
        self.spaces = Vec::new();
        self.boards = Vec::new();
        self.invites = Vec::new();
    }

    /// Find a model by id in a collection of items
    pub fn finder<'a, T>(items: &'a mut Vec<T>, item_id: &String) -> Option<&'a mut T>
        where T: Model
    {
        items.iter_mut()
            .filter(|x| x.id() == Some(item_id))
            .next()
    }
}

