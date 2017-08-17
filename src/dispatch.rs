//! Dispatch takes messages sent from our wonderful UI and runs the needed core
//! code to generate the response. Essentially, it's the RPC endpoint for core.
//!
//! Each message sent in is in the following format (JSON):
//! 
//!     ["<message id>", "<command>", arg1, arg2, ...]
//!
//! where the arg\* can be any valid JSON object. The Message ID is passed in
//! when responding so the client knows which request we are responding to.

use ::jedi::{self, Value};

use ::error::{TResult, TError};
use ::config;
use ::util;
use ::util::event::Emitter;
use ::turtl::Turtl;
use ::search::Query;
use ::profile::Profile;
use ::models::model::Model;
use ::models::protected::Protected;
use ::models::keychain::KeychainEntry;
use ::models::user::User;
use ::models::space::Space;
use ::models::space_member::SpaceMember;
use ::models::board::Board;
use ::models::note::Note;
use ::models::invite::{Invite, InviteRequest};
use ::models::file::FileData;
use ::models::sync_record::{SyncAction, SyncType, SyncRecord};
use ::models::feedback::Feedback;
use ::sync::sync_model::{self, MemorySaver};
use ::messaging::{self, Event};
use ::lib_permissions::Permission;
use ::models::storable::Storable;
use ::std::mem;

/// Does our actual message dispatching
fn dispatch(cmd: &String, turtl: &Turtl, data: Value) -> TResult<Value> {
    match cmd.as_ref() {
        "user:login" => {
            let username = jedi::get(&["2"], &data)?;
            let password = jedi::get(&["3"], &data)?;
            turtl.login(username, password)?;
            Ok(jedi::obj())
        },
        "user:join" => {
            let username = jedi::get(&["2"], &data)?;
            let password = jedi::get(&["3"], &data)?;
            turtl.join(username, password)?;
            Ok(jedi::obj())
        },
        "user:logout" => {
            turtl.logout()?;
            util::sleep(1000);
            Ok(jedi::obj())
        },
        "user:change-password" => {
            let current_username = jedi::get(&["2"], &data)?;
            let current_password = jedi::get(&["3"], &data)?;
            let new_username = jedi::get(&["4"], &data)?;
            let new_password = jedi::get(&["5"], &data)?;
            turtl.change_user_password(current_username, current_password, new_username, new_password)?;
            Ok(jedi::obj())
        },
        "user:delete-account" => {
            turtl.delete_account()?;
            Ok(jedi::obj())
        },
        "user:find-by-email" => {
            let email: String = jedi::get(&["2"], &data)?;
            let user = User::find_by_email(turtl, &email)?;
            Ok(jedi::to_val(&user)?)
        }
        "app:connected" => {
            let connguard = turtl.connected.read().unwrap();
            let connected: bool = *connguard;
            drop(connguard);
            Ok(Value::Bool(connected))
        },
        "app:wipe-user-data" => {
            turtl.wipe_user_data()?;
            Ok(jedi::obj())
        },
        "app:wipe-app-data" => {
            turtl.wipe_app_data()?;
            Ok(jedi::obj())
        },
        "sync:start" => {
            turtl.sync_start()?;
            Ok(jedi::obj())
        },
        "sync:pause" => {
            turtl.sync_pause();
            Ok(jedi::obj())
        },
        "sync:resume" => {
            turtl.sync_resume();
            Ok(jedi::obj())
        },
        "sync:shutdown" => {
            turtl.sync_shutdown(true)?;
            Ok(jedi::obj())
        },
        "sync:get-pending" => {
            let frozen = SyncRecord::get_all_pending(turtl)?;
            Ok(jedi::to_val(&frozen)?)
        },
        "sync:unfreeze-item" => {
            let sync_id: String = jedi::get(&["2"], &data)?;
            SyncRecord::kick_frozen_sync(turtl, &sync_id)?;
            Ok(jedi::obj())
        },
        "sync:delete-item" => {
            let sync_id: String = jedi::get(&["2"], &data)?;
            SyncRecord::delete_sync_item(turtl, &sync_id)?;
            Ok(jedi::obj())
        },
        "app:api:set-endpoint" => {
            let endpoint: String = jedi::get(&["2"], &data)?;
            config::set(&["api", "endpoint"], &endpoint)?;
            Ok(jedi::obj())
        },
        "app:shutdown" => {
            turtl.sync_shutdown(false)?;
            turtl.events.trigger("app:shutdown", &jedi::obj());
            Ok(jedi::obj())
        },
        "profile:load" => {
            let user_guard = turtl.user.read().unwrap();
            let profile_guard = turtl.profile.read().unwrap();
            let profile_data = json!({
                "user": &user_guard.as_ref(),
                "spaces": &profile_guard.spaces,
                "boards": &profile_guard.boards,
            });
            Ok(profile_data)
        },
        "profile:sync:model" => {
            let action: SyncAction = match jedi::get(&["2"], &data) {
                Ok(action) => action,
                Err(e) => return Err(TError::BadValue(format!("dispatch: {} -- bad sync action: {}", cmd, e))),
            };
            let ty: SyncType = jedi::get(&["3"], &data)?;

            match action.clone() {
                SyncAction::Add | SyncAction::Edit => {
                    let val = match ty {
                        SyncType::User => {
                            if action != SyncAction::Edit {
                                return Err(TError::BadValue(format!("dispatch: {} -- cannot `add` item of type {:?}", cmd, ty)));
                            }
                            let mut model: User = jedi::get(&["4"], &data)?;
                            sync_model::save_model(action, turtl, &mut model, false)?
                        }
                        SyncType::Space => {
                            let mut model: Space = jedi::get(&["4"], &data)?;
                            match &action {
                                &SyncAction::Edit => {
                                    let fake_id = String::from("<no id>");
                                    let space_id = model.id().unwrap_or(&fake_id);
                                    Space::permission_check(turtl, space_id, &Permission::EditSpace)?;
                                }
                                _ => {}
                            };
                            sync_model::save_model(action, turtl, &mut model, false)?
                        }
                        SyncType::Board => {
                            let mut model: Board = jedi::get(&["4"], &data)?;
                            let permission = match &action {
                                &SyncAction::Add => Permission::AddBoard,
                                &SyncAction::Edit => Permission::EditBoard,
                                _ => return Err(TError::BadValue(format!("dispatch: {} -- couldn't find permission for {:?}/{:?}", cmd, ty, action))),
                            };
                            Space::permission_check(turtl, &model.space_id, &permission)?;
                            sync_model::save_model(action, turtl, &mut model, false)?
                        }
                        SyncType::Note => {
                            let mut note: Note = jedi::get(&["4"], &data)?;
                            let permission = match &action {
                                &SyncAction::Add => Permission::AddNote,
                                &SyncAction::Edit => Permission::EditNote,
                                _ => return Err(TError::BadValue(format!("dispatch: {} -- couldn't find permission for {:?}/{:?}", cmd, ty, action))),
                            };
                            Space::permission_check(turtl, &note.space_id, &permission)?;
                            // always set to false. this is a public field that
                            // we let the server manage for us
                            note.has_file = false;
                            let filemebbe: Option<FileData> = jedi::get_opt(&["5"], &data);
                            let note_data = sync_model::save_model(action, turtl, &mut note, false)?;
                            match filemebbe {
                                Some(mut file) => {
                                    file.save(turtl, &mut note)?;
                                }
                                None => {}
                            }
                            note_data
                        }
                        _ => {
                            return Err(TError::BadValue(format!("dispatch: {} -- cannot direct sync an item of type {:?}", cmd, ty)));
                        }
                    };
                    Ok(val)
                }
                SyncAction::Delete => {
                    let id: String = jedi::get(&["4", "id"], &data)?;
                    fn get_model<T>(turtl: &Turtl, id: &String, cmd: &str) -> TResult<T>
                        where T: Protected + Storable
                    {
                        let mut db_guard = turtl.db.write().unwrap();
                        let db = match db_guard.as_mut() {
                            Some(x) => x,
                            None => return Err(TError::MissingField(format!("dispatch: {} -- turtl is missing `db` object", cmd))),
                        };
                        match db.get::<T>(T::tablename(), id)? {
                            Some(x) => Ok(x),
                            None => return Err(TError::NotFound(format!("dispatch: {} -- that {} model wasn't found", cmd, T::tablename()))),
                        }
                    }
                    match ty {
                        SyncType::Space => {
                            Space::permission_check(turtl, &id, &Permission::DeleteSpace)?;
                            sync_model::delete_model::<Space>(turtl, &id, false)?;
                        }
                        SyncType::Board => {
                            let model = get_model::<Board>(turtl, &id, cmd)?;
                            Space::permission_check(turtl, &model.space_id, &Permission::DeleteBoard)?;
                            sync_model::delete_model::<Board>(turtl, &id, false)?;
                        }
                        SyncType::Note => {
                            let model = get_model::<Note>(turtl, &id, cmd)?;
                            Space::permission_check(turtl, &model.space_id, &Permission::DeleteNote)?;
                            sync_model::delete_model::<Note>(turtl, &id, false)?;
                        }
                        SyncType::File => {
                            let model = get_model::<Note>(turtl, &id, cmd)?;
                            Space::permission_check(turtl, &model.space_id, &Permission::EditNote)?;
                            sync_model::delete_model::<FileData>(turtl, &id, false)?;
                        }
                        _ => {
                            return Err(TError::BadValue(format!("dispatch: {} -- cannot direct sync an item of type {:?}", cmd, ty)));
                        }
                    }
                    Ok(jedi::obj())
                }
                SyncAction::MoveSpace => {
                    let item_id = jedi::get(&["4", "id"], &data)?;
                    let space_id = jedi::get(&["4", "space_id"], &data)?;
                    match ty {
                        SyncType::Board => {
                            Space::permission_check(turtl, &space_id, &Permission::DeleteBoard)?;
                            Space::permission_check(turtl, &space_id, &Permission::AddBoard)?;
                            let mut profile_guard = turtl.profile.write().unwrap();
                            let boards = &mut profile_guard.boards;
                            let board = match Profile::finder(boards, &item_id) {
                                Some(m) => m,
                                None => return Err(TError::MissingData(format!("dispatch: {} -- cannot find Board {} in profile", cmd, item_id))),
                            };
                            board.move_spaces(turtl, space_id)?;
                        }
                        SyncType::Note => {
                            Space::permission_check(turtl, &space_id, &Permission::DeleteNote)?;
                            Space::permission_check(turtl, &space_id, &Permission::AddNote)?;
                            let mut notes = turtl.load_notes(&vec![item_id.clone()])?;
                            if notes.len() == 0 {
                                return Err(TError::MissingData(format!("dispatch: {} -- trouble grabbing Note {}", cmd, item_id)));
                            }
                            let note = &mut notes[0];
                            note.move_spaces(turtl, space_id)?;
                        }
                        _ => {
                            return Err(TError::BadValue(format!("dispatch: {} -- cannot {:?} item of type {:?}", cmd, action, ty)));
                        }
                    }
                    Ok(jedi::obj())
                }
                _ => {
                    Err(TError::BadValue(format!("dispatch: {} -- unimplemented sync action {:?}", cmd, action)))
                }
            }
        },
        "profile:space:set-owner" => {
            let space_id = jedi::get(&["2"], &data)?;
            let user_id = jedi::get(&["3"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, space_id))),
            };
            space.set_owner(turtl, &user_id)?;
            Ok(space.data()?)
        },
        "profile:space:edit-member" => {
            let mut member: SpaceMember = jedi::get(&["2"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &member.space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, member.space_id))),
            };
            space.edit_member(turtl, &mut member)?;
            Ok(space.data()?)
        },
        "profile:space:delete-member" => {
            let space_id: String = jedi::get(&["2"], &data)?;
            let user_id: String = jedi::get(&["3"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, space_id))),
            };
            space.delete_member(turtl, &user_id)?;
            Ok(space.data()?)
        },
        "profile:space:leave" => {
            let space_id: String = jedi::get(&["2"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, space_id))),
            };
            space.leave(turtl)?;
            Ok(space.data()?)
        },
        "profile:space:send-invite" => {
            let req: InviteRequest = jedi::get(&["2"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &req.space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, req.space_id))),
            };
            space.send_invite(turtl, req)?;
            Ok(space.data()?)
        },
        "profile:space:accept-invite" => {
            let space_id: String = jedi::get(&["2"], &data)?;
            let invite_id: String = jedi::get(&["3"], &data)?;
            let passphrase: Option<String> = jedi::get_opt(&["4"], &data);
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, space_id))),
            };
            space.accept_invite(turtl, &invite_id, passphrase)?;
            Ok(space.data()?)
        },
        "profile:space:edit-invite" => {
            let mut invite: Invite = jedi::get(&["2"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &invite.space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, invite.space_id))),
            };
            space.edit_invite(turtl, &mut invite)?;
            Ok(space.data()?)
        },
        "profile:space:delete-invite" => {
            let space_id: String = jedi::get(&["2"], &data)?;
            let invite_id: String = jedi::get(&["3"], &data)?;
            let mut profile_guard = turtl.profile.write().unwrap();
            let mut space = match Profile::finder(&mut profile_guard.spaces, &space_id) {
                Some(s) => s,
                None => return Err(TError::MissingData(format!("dispatch: {} -- couldn't find space {}", cmd, space_id))),
            };
            space.delete_invite(turtl, &invite_id)?;
            Ok(space.data()?)
        },
        "profile:get-notes" => {
            let note_ids = jedi::get(&["2"], &data)?;
            let notes: Vec<Note> = turtl.load_notes(&note_ids)?;
            Ok(jedi::to_val(&notes)?)
        },
        "profile:find-notes" => {
            let qry: Query = jedi::get(&["2"], &data)?;
            let search_guard = turtl.search.read().unwrap();
            if search_guard.is_none() {
                return Err(TError::MissingField(format!("dispatch: {} -- turtl is missing `search` object", cmd)));
            }
            let search = search_guard.as_ref().unwrap();
            let note_ids = search.find(&qry)?;
            let notes: Vec<Note> = turtl.load_notes(&note_ids)?;
            Ok(jedi::to_val(&notes)?)
        },
        "profile:get-file" => {
            let note_id = jedi::get(&["2"], &data)?;
            let notes: Vec<Note> = turtl.load_notes(&vec![note_id])?;
            FileData::load_file(turtl, &notes[0])?;
            Ok(Value::Null)
        },
        "profile:get-tags" => {
            let space_id: String = jedi::get(&["2"], &data)?;
            let boards: Vec<String> = jedi::get(&["3"], &data)?;
            let limit: i32 = jedi::get(&["4"], &data)?;
            let search_guard = turtl.search.read().unwrap();
            if search_guard.is_none() {
                return Err(TError::MissingField(format!("dispatch: {} -- turtl is missing `search` object", cmd)));
            }
            let search = search_guard.as_ref().unwrap();
            let tags = search.tags_by_frequency(&space_id, &boards, limit)?;
            Ok(jedi::to_val(&tags)?)
        },
        "feedback:send" => {
            let feedback: Feedback = jedi::get(&["2"], &data)?;
            feedback.send(turtl)?;
            Ok(jedi::obj())
        },
        "ping" => {
            info!("ping!");
            Ok(Value::String(String::from("pong")))
        },
        _ => {
            Err(TError::MissingCommand(cmd.clone()))
        }
    }
}

/// Event dispatching. This acts as a way for parts of the app that don't have
/// access to the Turtl object to trigger events.
fn dispatch_event(cmd: &String, turtl: &Turtl, data: Value) -> TResult<()> {
    info!("dispatch::dispatch_event() -- {}", cmd);
    match cmd.as_ref() {
        "sync:connected" => {
            let yesno: bool = jedi::from_val(data)?;
            let mut connguard = turtl.connected.write().unwrap();
            *connguard = yesno;
        }
        "sync:incoming" => {
            // implement MemorySaver for incoming syncs!
            let sync_item: SyncRecord = jedi::from_val(data)?;
            match sync_item.action.clone() {
                SyncAction::Add | SyncAction::Edit => {
                    fn load_save<T>(turtl: &Turtl, mut sync_item: SyncRecord) -> TResult<()>
                        where T: Protected + MemorySaver
                    {
                        let mut data = Value::Null;
                        match sync_item.data.as_mut() {
                            Some(x) => mem::swap(&mut data, x),
                            None => return Err(TError::MissingData(format!("dispatch_event: sync:incoming -- sync item missing `data` field."))),
                        }
                        let model: T = jedi::from_val(data)?;
                        model.save_to_mem(turtl)?;
                        Ok(())
                    }
                    match sync_item.ty.clone() {
                        SyncType::User => load_save::<User>(turtl, sync_item)?,
                        SyncType::Keychain => load_save::<KeychainEntry>(turtl, sync_item)?,
                        SyncType::Space => load_save::<Space>(turtl, sync_item)?,
                        SyncType::Board => load_save::<Board>(turtl, sync_item)?,
                        SyncType::Note => load_save::<Note>(turtl, sync_item)?,
                        SyncType::File => load_save::<FileData>(turtl, sync_item)?,
                        SyncType::Invite => load_save::<Invite>(turtl, sync_item)?,
                        _ => (),
                    }
                }
                SyncAction::Delete => {
                    fn load_delete<T>(turtl: &Turtl, sync_item: SyncRecord) -> TResult<()>
                        where T: Protected + MemorySaver
                    {
                        let mut model: T = Default::default();
                        model.set_id(sync_item.item_id.clone());
                        model.delete_from_mem(turtl)?;
                        Ok(())
                    }
                    match sync_item.ty.clone() {
                        SyncType::User => load_delete::<User>(turtl, sync_item)?,
                        SyncType::Keychain => load_delete::<KeychainEntry>(turtl, sync_item)?,
                        SyncType::Space => load_delete::<Space>(turtl, sync_item)?,
                        SyncType::Board => load_delete::<Board>(turtl, sync_item)?,
                        SyncType::Note => load_delete::<Note>(turtl, sync_item)?,
                        SyncType::File => load_delete::<FileData>(turtl, sync_item)?,
                        SyncType::Invite => load_delete::<Invite>(turtl, sync_item)?,
                        _ => (),
                    }
                }
                _ => {}
            }
        }
        "user:change-password:logout" => {
            messaging::ui_event("user:change-password:logout", &jedi::obj())?;
            util::sleep(3000);
            turtl.logout()?;
        }
        _ => {
            warn!("dispatch_event() -- encountered unknown event: {}", cmd);
        }
    }
    Ok(())
}

/// process a message from the messaging system. this is the main communication
/// heart of turtl core.
pub fn process(turtl: &Turtl, msg: &String) -> TResult<()> {
    if &msg[0..4] == "::ev" {
        let event: Event = jedi::parse(&String::from(&msg[4..]))?;
        let Event {e, d} = event;
        return dispatch_event(&e, turtl, d);
    }

    let data: Value = jedi::parse(msg)?;

    // grab the request id from the data
    let mid: String = match jedi::get(&["0"], &data) {
        Ok(x) => x,
        Err(_) => return Err(TError::MissingField(String::from("missing mid (0)"))),
    };
    // grab the command from the data
    let cmd: String = match jedi::get(&["1"], &data) {
        Ok(x) => x,
        Err(_) => return Err(TError::MissingField(String::from("missing cmd (1)"))),
    };

    info!("dispatch({}): {}", mid, cmd);

    match dispatch(&cmd, turtl.clone(), data) {
        Ok(val) => {
            match turtl.msg_success(&mid, val) {
                Err(e) => error!("dispatch::process() -- problem sending response (mid {}): {}", mid, e),
                _ => {},
            }
        },
        Err(e) => {
            match turtl.msg_error(&mid, &e) {
                Err(e) => error!("dispatch:process() -- problem sending (error) response (mod {}): {}", mid, e),
                _ => {},
            }
        },
    }
    Ok(())
}

