extern crate actix;
extern crate actix_lua;
extern crate actix_web;
extern crate futures;
#[macro_use]
extern crate lazy_static;
extern crate serde_json;

use actix::prelude::*;
use actix_lua::{LuaActor, LuaActorBuilder, LuaMessage};
use actix_web::actix::ContextFutureSpawner;
use actix_web::actix::{Actor, Context, System, WrapFuture};
use actix_web::client;
use actix_web::HttpMessage;
use futures::future::Future;
use serde_json::Value;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Mutex;
use std::time::Duration;

const API_HOST: &str = "http://www.pathofexile.com/api/public-stash-tabs";

lazy_static! {
    static ref ON_ADD: Addr<LuaActor> = {
        LuaActorBuilder::new()
            .on_handle_with_lua(
                r#"
            local json = require("json")
            if ctx.msg == 0 then
                print("reloading")
                package.loaded["on_add"] = nil
                return 0
            end

            local handler = require("on_add")
            local data = json.decode(ctx.msg)
            handler(data)
            return 0
            "#,
            ).build()
            .unwrap()
            .start()
    };
    static ref ON_REMOVE: Addr<LuaActor> = {
        LuaActorBuilder::new()
            .on_handle_with_lua(
                r#"
            -- print('lua remove')
            -- ignore it for now.
            return 0
            "#,
            ).build()
            .unwrap()
            .start()
    };
    static ref STASHES: Mutex<HashSet<String>> = { Mutex::new(HashSet::new()) };
    static ref ITEMS: Mutex<HashSet<String>> = { Mutex::new(HashSet::new()) };
}

struct Poll(String);

impl Message for Poll {
    type Result = ();
}

#[derive(Default)]
struct ParseStash {}

impl Actor for ParseStash {
    type Context = Context<Self>;
}

impl actix::Supervised for ParseStash {}
impl SystemService for ParseStash {}

struct StashMessage(Value);

impl Message for StashMessage {
    type Result = ();
}

impl Handler<StashMessage> for ParseStash {
    type Result = ();

    fn handle(&mut self, msg: StashMessage, _ctx: &mut Context<Self>) -> Self::Result {
        let v = msg.0;

        let mut stashes = STASHES.lock().unwrap();
        for stash in v["stashes"].as_array().unwrap() {
            let stash_id = stash["id"].as_str().unwrap();
            if let Some(_) = stashes.get(stash_id) {
                // diff
                let mut next_items = HashSet::new();
                for item in stash["items"].as_array().unwrap() {
                    let item_key =
                        format!("{}/{}", stash_id.to_string(), item["id"].as_str().unwrap());
                    next_items.insert(item_key);
                }

                let mut items = ITEMS.lock().unwrap();
                let clone = items.clone();
                let new_items: HashSet<&String> = HashSet::from_iter(next_items.difference(&clone));
                let removed_items: HashSet<&String> =
                    HashSet::from_iter(clone.difference(&next_items));

                let mut new_item_stash = stash.clone();
                let mut new_item_stash_items = Vec::<Value>::new();

                let mut removed_item_stash = stash.clone();
                let mut removed_item_stash_items = Vec::<Value>::new();

                for item in stash["items"].as_array().unwrap() {
                    let item_key =
                        format!("{}/{}", stash_id.to_string(), item["id"].as_str().unwrap());

                    if let Some(_) = new_items.get(&item_key) {
                        new_item_stash_items.push(item.clone());
                    }

                    if let Some(_) = removed_items.get(&item_key) {
                        removed_item_stash_items.push(item.clone());
                    }
                }

                if new_item_stash_items.len() > 0 {
                    new_item_stash["items"] = Value::from(new_item_stash_items);
                    ON_ADD.do_send(LuaMessage::from(new_item_stash.to_string()));
                }

                if removed_item_stash_items.len() > 0 {
                    removed_item_stash["items"] = Value::from(removed_item_stash_items);
                    ON_REMOVE.do_send(LuaMessage::from(removed_item_stash.to_string()));
                }

                for item_key in removed_items {
                    items.remove(item_key);
                }
            } else {
                stashes.insert(stash_id.to_string());
                let mut items = ITEMS.lock().unwrap();
                for item in stash["items"].as_array().unwrap() {
                    let item_key =
                        format!("{}/{}", stash_id.to_string(), item["id"].as_str().unwrap());
                    items.insert(item_key);
                }
                ON_ADD.do_send(LuaMessage::from(stash.to_string()));
            }
        }

        // reload script after every batch
        ON_ADD.do_send(LuaMessage::from(0));

        println!("tracked stashes: {}", stashes.len());
    }
}

#[derive(Default)]
struct PollActor {}

impl Actor for PollActor {
    type Context = Context<Self>;
}

impl actix::Supervised for PollActor {}
impl SystemService for PollActor {}

impl Handler<Poll> for PollActor {
    type Result = ();

    fn handle(&mut self, msg: Poll, ctx: &mut Context<Self>) -> Self::Result {
        let url = format!("{}?id={}", API_HOST, msg.0);
        println!("polling {}", url);
        client::get(url)
            .header("User-Agent", "Actix-web")
            .timeout(Duration::new(30, 0))
            .finish()
            .unwrap()
            .send()
            .map_err(|e| panic!("request error {:?}", e))
            .and_then(move |response| {
                response.body().limit(10 * 1024 * 1024).map_err(|e| {
                    panic!("request body error {:?}", e);
                })
            }).and_then(|body| {
                let v: Value = serde_json::from_slice(&body).unwrap();
                println!("next: {}", v["next_change_id"]);

                let act = System::current().registry().get::<ParseStash>();
                act.do_send(StashMessage(v.clone()));

                let act = System::current().registry().get::<PollActor>();
                act.do_send(Poll(v["next_change_id"].as_str().unwrap().to_string()));

                Ok(())
            }).into_actor(self)
            .wait(ctx);
    }
}

struct Bootstrap {}

impl Actor for Bootstrap {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        client::get("https://poe.ninja/api/Data/GetStats")
            .header("User-Agent", "Actix-web")
            .timeout(Duration::new(30, 0))
            .finish()
            .unwrap()
            .send()
            .map_err(|e| panic!("error {:?}", e))
            .and_then(move |response| {
                response.body().limit(10 * 1024 * 1024).map_err(|e| {
                    panic!("request body error {:?}", e);
                })
            }).and_then(|body| {
                let v: Value = serde_json::from_slice(&body).unwrap();

                let next_change_id = v["next_change_id"].as_str().unwrap().to_string();
                println!("starting from offset: {}", next_change_id);

                let act = System::current().registry().get::<PollActor>();
                act.do_send(Poll(next_change_id));
                Ok(())
            }).into_actor(self)
            .wait(ctx);
    }
}

fn main() {
    System::run(|| {
        PollActor {}.start();
        Bootstrap {}.start();
        ParseStash {}.start();
    });
}
