use std::convert::TryFrom;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::{env, fs, process::exit};

use bytes::buf::Buf;
use ipfs_api::{IpfsClient, TryFromUri};
use matrix_sdk::{
    self,
    events::collections::all::RoomEvent,
    events::room::{
        member::MemberEventContent,
        message::{MessageEvent, MessageEventContent, NoticeMessageEventContent, RelatesTo},
    },
    events::stripped::StrippedRoomMember,
    identifiers::{RoomId, UserId},
    Client, ClientConfig, EventEmitter, Session as SDKSession, SyncRoom, SyncSettings,
};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;
use url::Url;

use crate::config::Config;
use crate::utils::{get_media_download_url, Session};

mod config;
mod get_room_event;
mod utils;

struct CommandBot {
    /// This clone of the `Client` will send requests to the server,
    /// while the other keeps us in sync with the server using `sync_forever`.
    client: Client,
    ipfs_client: IpfsClient,
    config: Config,
}

impl CommandBot {
    pub fn new(client: Client, config: Config) -> Self {
        let ipfs_client = IpfsClient::from_str(&config.ipfs_api).unwrap();
        Self {
            client,
            ipfs_client,
            config,
        }
    }

    fn get_temp_file(&self, filename: String) -> PathBuf {
        let tmp_dir = env::temp_dir();
        tmp_dir.join(filename)
    }
    fn save_file(&self, filename: String, body: &[u8]) {
        println!("file to download: '{}'", filename);
        println!("will be located under: '{:?}'", &filename);
        let filename = self.get_temp_file(filename);
        let mut dest = { File::create(&filename).unwrap() };

        dest.write_all(body).unwrap();
    }

    fn remove_file(&self, filename: String) {
        let filename = self.get_temp_file(filename);
        fs::remove_file(filename).unwrap();
    }

    async fn send_link(
        &self,
        room_id: &RoomId,
        filename: String,
        hash: String,
        related_event_original: Option<RelatesTo>,
    ) {
        let content = MessageEventContent::Notice(NoticeMessageEventContent {
            body: format!(
                "{}/ipfs/{}?filename={}",
                self.config.ipfs_gateway, hash, filename
            ),
            format: None,
            formatted_body: None,
            relates_to: related_event_original,
        });

        self.client
            // send our message to the room we found the "!party" command in
            // the last parameter is an optional Uuid which we don't care about.
            .room_send(room_id, content, None)
            .await
            .unwrap();
    }

    async fn handle_media(&self, mxc_url: String, raw_filename: String) -> String {
        let download_url = get_media_download_url(self.client.homeserver(), mxc_url);

        let response = reqwest::get(&download_url).await.unwrap();

        let content = response.bytes().await.unwrap();

        self.save_file(raw_filename.clone(), content.bytes());

        let filename = self.get_temp_file(raw_filename.clone());
        let ipfs_resp = self.ipfs_client.add_path(&filename).await.unwrap();
        self.remove_file(raw_filename);

        let hash = ipfs_resp.first().unwrap().hash.clone();
        self.ipfs_client.pin_add(&hash, true).await.unwrap();

        hash
    }
}

#[matrix_sdk_common_macros::async_trait]
impl EventEmitter for CommandBot {
    async fn on_stripped_state_member(
        &self,
        room: SyncRoom,
        _: &StrippedRoomMember,
        _: Option<MemberEventContent>,
    ) {
        if let SyncRoom::Invited(room) = room {
            let room_id = room.read().await.room_id.clone();
            let resp = self.client.join_room_by_id(&room_id).await.unwrap();
        }
    }
    async fn on_room_message(&self, room: SyncRoom, event: &MessageEvent) {
        if let SyncRoom::Joined(room) = room {
            if let MessageEventContent::Text(text_event) = event.clone().content {

                let msg_body = text_event.body.clone();

                // TODO fix e2ee relates_to with something like https://github.com/matrix-org/matrix-rust-sdk/blob/master/matrix_sdk_base/src/client.rs#L93 inside of receive_joined_timeline_event

                if msg_body.contains("!ipfs") && text_event.relates_to.is_some() {
                    println!("new !ipfs message");
                    let related_event_original = text_event.relates_to.clone();

                    // we clone here to hold the lock for as little time as possible.
                    let room_id = room.read().await.room_id.clone();
                    let mut related_events: Vec<MessageEvent> = room
                        .read()
                        .await
                        .messages
                        .iter()
                        .filter(|x| {
                            (**x).event_id
                                == related_event_original
                                    .as_ref()
                                    .unwrap()
                                    .in_reply_to
                                    .event_id
                        })
                        .map(|x| (**x).clone())
                        .collect();
                    if related_events.is_empty() {
                        // Fetch missing event
                        let resp = self
                            .client
                            .send(get_room_event::Request {
                                room_id: room_id.clone(),
                                event_id: related_event_original
                                    .clone()
                                    .unwrap()
                                    .in_reply_to
                                    .event_id,
                            })
                            .await;

                        match resp {
                            Ok(mut resp) => {
                                println!("{:?}", resp.event.deserialize());
                                let (event, _updated) = self
                                    .client
                                    .base_client
                                    .receive_joined_timeline_event(&room_id, &mut resp.event)
                                    .await
                                    .unwrap();
                                match event {
                                    Some(event) => {
                                        if let Ok(RoomEvent::RoomMessage(msg_event)) =
                                            event.deserialize()
                                        {
                                            related_events.push(msg_event);
                                        }
                                    }
                                    None => {
                                        if let Ok(RoomEvent::RoomMessage(msg_event)) =
                                            resp.event.deserialize()
                                        {
                                            related_events.push(msg_event);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                println!("error: {:?}", e);
                            }
                        }
                    }
                    if !related_events.is_empty() {
                        let related_event = related_events.first();

                        if let Some(related_event) = related_event {
                            // TODO handle media content
                            info!("got related_event");

                            match related_event.clone().content {
                                MessageEventContent::Image(image_event) => {
                                    info!("handling image event");

                                    // Saving image
                                    let filename = image_event.body.clone();
                                    let hash = match image_event.url {
                                        None => {
                                            self.handle_media(
                                                image_event.file.unwrap().url,
                                                filename.clone(),
                                            )
                                            .await
                                        }
                                        Some(url) => self.handle_media(url, filename.clone()).await,
                                    };

                                    // Sending link
                                    self.send_link(
                                        &room_id,
                                        filename.clone(),
                                        hash,
                                        related_event_original.clone(),
                                    )
                                    .await;

                                    info!("image event message sent");
                                }
                                MessageEventContent::Video(video_event) => {
                                    info!("handling video event");

                                    // Saving video
                                    let filename = video_event.body.clone();
                                    let hash = match video_event.url {
                                        None => {
                                            self.handle_media(
                                                video_event.file.unwrap().url,
                                                filename.clone(),
                                            )
                                            .await
                                        }
                                        Some(url) => self.handle_media(url, filename.clone()).await,
                                    };

                                    // Sending link
                                    self.send_link(
                                        &room_id,
                                        filename.clone(),
                                        hash,
                                        related_event_original.clone(),
                                    )
                                    .await;

                                    info!("video event message sent");
                                }
                                MessageEventContent::File(file_event) => {
                                    info!("handling file event");

                                    // Saving file
                                    let filename = file_event.body.clone();
                                    let hash = match file_event.url {
                                        None => {
                                            self.handle_media(
                                                file_event.file.unwrap().url,
                                                filename.clone(),
                                            )
                                            .await
                                        }
                                        Some(url) => self.handle_media(url, filename.clone()).await,
                                    };

                                    // Sending link
                                    self.send_link(
                                        &room_id,
                                        filename.clone(),
                                        hash,
                                        related_event_original.clone(),
                                    )
                                    .await;

                                    info!("file event message sent");
                                }
                                MessageEventContent::Audio(audio_event) => {
                                    info!("handling audio event");

                                    // Saving audio
                                    let filename = audio_event.body.clone();
                                    let hash = match audio_event.url {
                                        None => {
                                            self.handle_media(
                                                audio_event.file.unwrap().url,
                                                filename.clone(),
                                            )
                                            .await
                                        }
                                        Some(url) => self.handle_media(url, filename.clone()).await,
                                    };

                                    // Sending link
                                    self.send_link(
                                        &room_id,
                                        filename.clone(),
                                        hash,
                                        related_event_original.clone(),
                                    )
                                    .await;

                                    info!("audio event message sent");
                                }
                                _ => {
                                    info!("sending fallback response");

                                    let content = MessageEventContent::Notice(NoticeMessageEventContent {
                                        body: "Only Image, Video, File and Audio events are supported!".to_string(),
                                        format: None,
                                        formatted_body: None,
                                        relates_to: related_event_original.clone(),
                                    });

                                    self.client
                                        // send our message to the room we found the "!party" command in
                                        // the last parameter is an optional Uuid which we don't care about.
                                        .room_send(&room_id, content, None)
                                        .await
                                        .unwrap();

                                    info!("fallback response message sent");
                                }
                            }
                        }
                    } else {
                        let content = MessageEventContent::Notice(NoticeMessageEventContent {
                            body: "Unable to find related event!".to_string(),
                            format: None,
                            formatted_body: None,
                            relates_to: related_event_original.clone(),
                        });

                        self.client
                            // send our message to the room we found the "!party" command in
                            // the last parameter is an optional Uuid which we don't care about.
                            .room_send(&room_id, content, None)
                            .await
                            .unwrap();

                        warn!("Unable to find related_event");
                    }
                }
            }
        }
    }
}

async fn login_and_sync(
    homeserver_url: String,
    username: String,
    password: String,
) -> Result<(), matrix_sdk::Error> {
    // the location for `JsonStore` to save files to
    let mut home = dirs::home_dir().expect("no home directory found");
    home.push("ipfs_bot");
    fs::create_dir_all(&home).unwrap();

    let client_config = ClientConfig::new()
        .store_path(&home)
        .passphrase(password.clone());

    let homeserver_url = Url::parse(&homeserver_url).expect("Couldn't parse the homeserver URL");
    // create a new Client with the given homeserver url and config
    let mut client = Client::new_with_config(homeserver_url, client_config).unwrap();

    let mut session = home.clone();
    session.push("session.json");
    if session.exists() {
        let f = OpenOptions::new().read(true).open(&session).unwrap();
        let json: Session = serde_json::from_reader(f).expect("file should be proper JSON");
        let session = SDKSession {
            access_token: json.access_token,
            user_id: UserId::try_from(json.user_id).unwrap(),
            device_id: json.device_id,
        };
        client.restore_login(session).await.unwrap();
    } else {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&session)
            .unwrap();

        let login_response = client
            .login(
                username.clone(),
                password,
                None,
                Some("ipfs bot".to_string()),
            )
            .await?;

        let session = Session {
            access_token: login_response.access_token,
            user_id: login_response.user_id.to_string(),
            device_id: login_response.device_id,
        };

        serde_json::to_writer(&f, &session).unwrap();
    }

    println!("logged in as {}", username);

    // add our CommandBot to be notified of incoming messages, we do this after the initial
    // sync to avoid responding to messages before the bot was running.
    client
        .add_event_emitter(Box::new(CommandBot::new(client.clone(), Config::load())))
        .await;

    // since we called sync before we `sync_forever` we must pass that sync token to
    // `sync_forever`
    let settings = SyncSettings::default();
    // this keeps state from the server streaming in to CommandBot via the EventEmitter trait
    client.sync_forever(settings, |_| async {}).await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), matrix_sdk::Error> {
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(Level::DEBUG)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let (homeserver_url, username, password) =
        match (env::args().nth(1), env::args().nth(2), env::args().nth(3)) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            _ => {
                eprintln!(
                    "Usage: {} <homeserver_url> <username> <password>",
                    env::args().next().unwrap()
                );
                exit(1)
            }
        };

    login_and_sync(homeserver_url, username, password).await?;
    Ok(())
}
