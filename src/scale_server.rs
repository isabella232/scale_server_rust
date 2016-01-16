use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;

extern crate url;

extern crate websocket;
use self::websocket::{Server, Message, DataFrame};
use self::websocket::message::Type;
use self::websocket::result::WebSocketResult;
use self::websocket::server::{Connection as WsConnection, Sender, Receiver};
use self::websocket::stream::WebSocketStream;
use self::websocket::ws::sender::Sender as WsSender;
use self::websocket::ws::receiver::Receiver as WsReceiver;

extern crate rustc_serialize;
use rustc_serialize::base64::{self, ToBase64};

use scale_listener::ScaleListener;

type ConnectionId = usize;
type ScaleServerRef = Arc<Mutex<ScaleServer>>;

type Client = websocket::client::Client<DataFrame,
                                        Sender<WebSocketStream>,
                                        Receiver<WebSocketStream>>;

struct Connection {
    id: ConnectionId,
    filter_scale_ids: Option<Vec<String>>,
    last_message_sent: Option<String>,
    sender: Sender<WebSocketStream>,
}

impl fmt::Display for Connection {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let _ = write!(formatter, "Connection(id={}, filter_scale_ids=", self.id);
        match self.filter_scale_ids.as_ref() {
            Some(ids) => write!(formatter, "{:?})", ids),
            None => write!(formatter, "None)"),
        }
    }
}

impl Connection {
    fn new(id: ConnectionId,
           filter_scale_ids: Option<Vec<String>>,
           sender: Sender<WebSocketStream>)
           -> Connection {
        Connection {
            id: id,
            filter_scale_ids: filter_scale_ids,
            last_message_sent: None,
            sender: sender,
        }
    }

    fn matches_filter(&self, scale_id: &str) -> bool {
        self.filter_scale_ids.as_ref().map_or(true, |ids| ids.contains(&scale_id.to_string()))
    }

    fn is_duplicate(&self, message: &str) -> bool {
        self.last_message_sent.as_ref().map_or(false, |last| last == message)
    }
}

pub struct ScaleServer {
    scale_listener: Arc<Mutex<ScaleListener>>,
    connections: HashMap<ConnectionId, Connection>,
    next_connection_id: ConnectionId,
}

impl ScaleServer {
    fn new(scale_listener: Arc<Mutex<ScaleListener>>) -> ScaleServer {
        ScaleServer {
            scale_listener: scale_listener,
            connections: HashMap::new(),
            next_connection_id: 0,
        }
    }

    pub fn start(scale_listen_addr: &str, websocket_listen_addr: &str) -> Result<(), io::Error> {
        let scale_listener = try!(ScaleListener::listen(scale_listen_addr));
        let scale_server = Arc::new(Mutex::new(ScaleServer::new(scale_listener)));

        ScaleServer::start_heartbeat(scale_server.clone());
        ScaleServer::observer_scale_listener(scale_server.clone());
        ScaleServer::start_websocket_server(scale_server.clone(), websocket_listen_addr)
    }

    fn start_websocket_server(scale_server: ScaleServerRef,
                              websocket_listen_addr: &str)
                              -> Result<(), io::Error> {
        let mut server = try!(Server::bind(websocket_listen_addr));

        loop {
            let scale_server = scale_server.clone();
            let connection = try!(server.accept());

            match ScaleServer::accept_websocket_connection(scale_server, connection) {
                Ok(_) => (),
                Err(err) => error!("error establishing connection: {}", err),
            }
        }
    }

    fn accept_websocket_connection(scale_server: ScaleServerRef,
                                   connection: WsConnection<WebSocketStream, WebSocketStream>)
                                   -> WebSocketResult<ConnectionId> {

        let request = try!(connection.read_request());
        let url = try!(url::Url::parse(&format!("ws://localhost{}", request.url)));
        let client = try!(request.accept().send());
        let (sender, mut receiver) = client.split();

        let connection_id = scale_server.lock()
                                        .unwrap()
                                        .add_connection(sender, ScaleServer::parse_scale_ids(&url));

        thread::spawn(move || {
            let scale_server = scale_server.clone();

            for message in receiver.incoming_messages() {
                let message: Message = match message {
                    Ok(message) => message,
                    Err(err) => {
                        error!("error reading from connection {}: {}", connection_id, err);
                        break;
                    }
                };

                match message.opcode {
                    Type::Ping => (),
                    Type::Close => break,
                    _ => scale_server.lock().unwrap().clear_message_history(&connection_id),
                }
            }

            scale_server.lock().unwrap().remove_connection(&connection_id);
        });

        Ok(connection_id)
    }

    fn start_heartbeat(scale_server: ScaleServerRef) {
        let scale_server = scale_server.clone();
        thread::spawn(move || {
            loop {
                scale_server.lock().unwrap().send_heartbeats();
                thread::sleep_ms(1000);
            }
        });
    }

    fn observer_scale_listener(scale_server: ScaleServerRef) {
        let (id, rx) = {
            let scale_server = scale_server.lock().unwrap();
            let mut listener = scale_server.scale_listener.lock().unwrap();
            listener.add_observer()
        };

        let scale_server = scale_server.clone();
        thread::spawn(move || {
            for message in rx {
                scale_server.lock().unwrap().forward_message(&message);
            }

            let scale_server = scale_server.lock().unwrap();
            scale_server.scale_listener.lock().unwrap().remove_observer(&id);
        });
    }

    fn parse_scale_ids(url: &url::Url) -> Option<Vec<String>> {
        let mut filter_scale_ids = Vec::<String>::new();
        url.query_pairs().map(|pairs| {
            for (key, val) in pairs {
                if key == "ids" {
                    filter_scale_ids.append(&mut val.split(",").map(str::to_string).collect());
                }
            }
        });

        if filter_scale_ids.len() > 0 {
            Some(filter_scale_ids)
        } else {
            None
        }
    }

    fn extract_message(message: &str) -> Option<(&str, &str)> {
        let splits = message.split("\x02").collect::<Vec<&str>>();
        if splits.len() == 2 {
            Some((splits[0], splits[1]))
        } else {
            None
        }
    }

    fn add_connection(&mut self,
                      sender: Sender<WebSocketStream>,
                      filter_scale_ids: Option<Vec<String>>)
                      -> ConnectionId {
        let id = self.next_connection_id;
        let connection = Connection::new(id, filter_scale_ids, sender);
        info!("open {}", &connection);
        self.connections.insert(id, connection);
        self.next_connection_id += 1;
        id
    }

    fn remove_connection(&mut self, connection_id: &ConnectionId) {
        for connection in self.connections.remove(connection_id) {
            info!("close conn_id={}", &connection);
        }
    }

    fn forward_message(&mut self, message: &str) {
        let mut erred_connections = vec![];
        let (scale_id, message) = match ScaleServer::extract_message(message) {
            Some(val) => val,
            _ => return,
        };

        for (id, connection) in self.connections.iter_mut() {
            if connection.matches_filter(scale_id) && !connection.is_duplicate(message) {
                let message_json = ScaleServer::message_to_json(scale_id, message);
                match connection.sender.send_message(&Message::text(message_json)) {
                    Err(_) => erred_connections.push(*id),
                    Ok(_) => (),
                }
                connection.last_message_sent = Some(message.to_string());
            }
        }

        for id in erred_connections {
            self.remove_connection(&id);
        }
    }

    fn message_to_json(scale_id: &str, message: &str) -> String {
        format!("{{\"scaleId\":\"{}\",\"data\":\"{}\"}}",
                scale_id,
                message.as_bytes().to_base64(base64::STANDARD))
    }

    fn clear_message_history(&mut self, client_id: &ConnectionId) {
        for connection in self.connections.get_mut(client_id) {
            connection.last_message_sent = None;
        }
    }

    fn send_heartbeats(&mut self) {
        let mut remove_ids = Vec::new();

        for (id, connection) in self.connections.iter_mut() {
            if connection.sender.send_message(&Message::text("")).is_err() {
                remove_ids.push(*id);
            }
        }

        for id in remove_ids.iter() {
            self.remove_connection(&id);
        }
    }
}
