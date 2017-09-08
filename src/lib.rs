#![recursion_limit="128"]

extern crate carrier;
extern crate clippo;
extern crate clouseau;
extern crate config;
extern crate crossbeam;
extern crate dumpy;
extern crate fern;
extern crate futures;
extern crate futures_cpupool;
extern crate glob;
extern crate hyper;
extern crate jedi;
#[macro_use]
extern crate lazy_static;
extern crate lib_permissions;
#[macro_use]
extern crate log;
//extern crate migrate;
extern crate num_cpus;
#[macro_use]
extern crate protected_derive;
#[macro_use]
extern crate quick_error;
extern crate regex;
extern crate rusqlite;
extern crate rustc_serialize as serialize;  // for hex/base64
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate sodiumoxide;
extern crate time;

#[macro_use]
pub mod error;
#[macro_use]
mod util;
mod crypto;
mod messaging;
mod api;
#[macro_use]
mod sync;
#[macro_use]
mod models;
mod profile;
mod storage;
mod search;
mod dispatch;
mod schema;
mod turtl;

use ::std::thread;
use ::std::sync::Arc;
use ::std::os::raw::c_char;
use ::std::ffi::CStr;
use ::std::panic;

use ::jedi::Value;

use ::error::TResult;
use ::util::event::Emitter;

/// Init any state/logging/etc the app needs
pub fn init() -> TResult<()> {
    match util::logger::setup_logger() {
        Ok(..) => Ok(()),
        Err(e) => Err(toterr!(e)),
    }
}

/// This takes a JSON-encoded object, and parses out the values we care about,
/// and populates them into our app-wide config (overwriting any values we may
/// have set in config.yaml).
fn process_runtime_config(config_str: String) -> TResult<()> {
    let runtime_config: Value = match jedi::parse(&config_str) {
        Ok(x) => x,
        Err(_) => jedi::obj(),
    };
    let data_folder: String = match jedi::get(&["data_folder"], &runtime_config) {
        Ok(x) => x,
        Err(_) => String::from("/tmp/"),
    };
    config::set(&["data_folder"], &data_folder)?;
    Ok(())
}

/// Start our app...spawns all our worker/helper threads, including our comm
/// system that listens for external messages.
///
/// NOTE: we have two configs. Our runtime config, which is passed in as a JSON
/// string to start(), and our app config that is loaded on init from our
/// config.yaml file. The runtime config is meant to set up things that will be
/// platform dependent and our UI will most likely have before it even starts
/// the Turtl core.
/// NOTE: we copy the runtime config into our main config, overwriting any of
/// those keys that exist in the config.yaml (app config). this gives the entire
/// app access to our runtime config.
pub fn start(config_str: String) -> thread::JoinHandle<()> {
    thread::Builder::new().name(String::from("turtl-main")).spawn(move || {
        let runner = move || -> TResult<()> {
            // load our ocnfiguration
            process_runtime_config(config_str)?;

            let data_folder = config::get::<String>(&["data_folder"])?;
            if data_folder != ":memory:" {
                util::create_dir(&data_folder)?;
                info!("main::start() -- created data folder: {}", data_folder);
            }

            // create our turtl object
            let turtl = Arc::new(turtl::Turtl::new()?);
            turtl.events.bind("app:shutdown", |_| {
                messaging::stop();
            }, "app:shutdown:main");

            // start our messaging thread
            let msg_res = messaging::start(move |msg: String| {
                let turtl2 = turtl.clone();
                let res = thread::Builder::new().name(String::from("dispatch:msg")).spawn(move || {
                    match dispatch::process(turtl2.as_ref(), &msg) {
                        Ok(..) => {},
                        Err(e) => error!("dispatch::process() -- error processing: {}", e),
                    }
                });
                match res {
                    Ok(..) => {},
                    Err(e) => error!("main::start() -- message processor: error spawning thread: {}", e),
                }
            });
            match msg_res {
                Ok(..) => {},
                Err(e) => error!("main::start() -- messaging error: {}", e),
            }
            info!("main::start() -- shutting down");
            Ok(())
        };
        match runner() {
            Ok(_) => (),
            Err(e) => {
                error!("main::start() -- {}", e);
            }
        }
    }).unwrap()
}

// -----------------------------------------------------------------------------
// our C api
// -----------------------------------------------------------------------------

/// Start Turtl
#[no_mangle]
pub extern fn turtl_start(config_c: *const c_char) -> i32 {
    let res = panic::catch_unwind(|| -> i32 {
        if config_c.is_null() { return -1; }
        let config_res = unsafe { CStr::from_ptr(config_c).to_str() };
        let config = match config_res {
            Ok(x) => x,
            Err(e) => {
                println!("turtl_start() -- error: parsing config: {}", e);
                return -3;
            },
        };
        match init() {
            Ok(_) => (),
            Err(e) => {
                println!("turtl_start() -- error: init(): {}", e);
                return -3;
            },
        }

        match start(String::from(&config[..])).join() {
            Ok(_) => (),
            Err(e) => {
                println!("turtl_start() -- error: start().join(): {:?}", e);
                return -4;
            },
        }
        0
    });
    match res {
        Ok(x) => x,
        Err(e) => {
            println!("turtl_start() -- panic: {:?}", e);
            return -5;
        },
    }
}

