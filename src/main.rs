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

// TODO: implement proper poe api types
// TODO: implement PartialEq, Eq, Hash on Item type with stash_key-item_key

const API_HOST: &str = "http://www.pathofexile.com/api/public-stash-tabs";

lazy_static! {
    static ref ON_ADD: Addr<LuaActor> = {
        LuaActorBuilder::new()
            .on_handle_with_lua(
                r#"
            -- print('lua add ' .. string.len(ctx.msg))
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
            print('lua remove ' .. ctx.msg)
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

struct StashMessage(String);

impl Message for StashMessage {
    type Result = ();
}

impl Handler<StashMessage> for ParseStash {
    type Result = ();

    fn handle(&mut self, msg: StashMessage, ctx: &mut Context<Self>) -> Self::Result {}
}

#[derive(Default)]
struct RequestActor {}

impl Actor for RequestActor {
    type Context = Context<Self>;
}

impl actix::Supervised for RequestActor {}
impl SystemService for RequestActor {}

impl Handler<Poll> for RequestActor {
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
            .map_err(|_| ())
            .and_then(move |response| {
                // println!("Response: {:?}", response);
                response.body().limit(10 * 1024 * 1024).map_err(|e| {
                    println!("request body error {:?}", e);
                    ()
                })
            }).and_then(|body| {
                // let b = format!("{}", body);
                // println!("{:?}", &b.to_string()[..100]);
                let v: Value = serde_json::from_slice(&body).unwrap();
                println!("Body: {:?}", body.len());
                println!("next: {:?}", v["next_change_id"]);

                // TODO: move to another actor
                let mut stashes = STASHES.lock().unwrap();
                for stash in v["stashes"].as_array().unwrap() {
                    let stash_id = stash["id"].as_str().unwrap();
                    if let Some(_) = stashes.get(stash_id) {
                        // diff
                        let mut next_items = HashSet::new();
                        for item in stash["items"].as_array().unwrap() {
                            let item_key = format!(
                                "{}/{}",
                                stash_id.to_string(),
                                item["id"].as_str().unwrap()
                            );
                            next_items.insert(item_key);
                        }

                        let mut items = ITEMS.lock().unwrap();
                        let clone = items.clone();
                        let new_items: HashSet<&String> =
                            HashSet::from_iter(next_items.difference(&clone));
                        let removed_items: HashSet<&String> =
                            HashSet::from_iter(clone.difference(&next_items));

                        // TODO: send new_item/remove_item to lua

                        let mut new_item_stash = stash.clone();
                        let mut new_item_stash_items = Vec::<Value>::new();

                        let mut removed_item_stash = stash.clone();
                        let mut removed_item_stash_items = Vec::<Value>::new();

                        for item in stash["items"].as_array().unwrap() {
                            let item_key = format!(
                                "{}/{}",
                                stash_id.to_string(),
                                item["id"].as_str().unwrap()
                            );

                            if let Some(_) = new_items.get(&item_key) {
                                // println!("new item on market: {:?}", item);
                                new_item_stash_items.push(item.clone());
                            }

                            if let Some(_) = removed_items.get(&item_key) {
                                // println!("remove item from market: {:?}", item);
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
                            let item_key = format!(
                                "{}/{}",
                                stash_id.to_string(),
                                item["id"].as_str().unwrap()
                            );
                            items.insert(item_key);
                        }
                        ON_ADD.do_send(LuaMessage::from(stash.to_string()));
                    }
                }

                println!("tracked stashes: {}", stashes.len());
                let act = System::current().registry().get::<RequestActor>();
                act.do_send(Poll(v["next_change_id"].as_str().unwrap().to_string()));
                // actix::System::current().stop();
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
            .map_err(|e| println!("error {:?}", e))
            .and_then(move |response| {
                println!("Response: {:?}", response);
                response.body().limit(10 * 1024 * 1024).map_err(|e| {
                    println!("request body error {:?}", e);
                    ()
                })
            }).and_then(|body| {
                let v: Value = serde_json::from_slice(&body).unwrap();
                println!("{}", String::from_utf8(body.to_vec()).unwrap());
                let act = System::current().registry().get::<RequestActor>();
                act.do_send(Poll(v["next_change_id"].as_str().unwrap().to_string()));
                Ok(())
            }).into_actor(self)
            .wait(ctx);
    }
}

fn main() {
    System::run(|| {
        RequestActor {}.start();
        Bootstrap {}.start();
    });
    // let system = System::new("test");

    // system.run();
}
