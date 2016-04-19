#[macro_use] extern crate log;
extern crate env_logger;
extern crate yxorp;
extern crate openssl;

use std::net::{UdpSocket,ToSocketAddrs};
use std::sync::mpsc::{channel};
use yxorp::network;
use yxorp::messages;
use yxorp::network::metrics::{METRICS,ProxyMetrics};
use openssl::ssl;

fn main() {
  env_logger::init().unwrap();
  info!("starting up");
  let metrics_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
  let metrics_host   = ("192.168.59.103", 8125).to_socket_addrs().unwrap().next().unwrap();
  METRICS.lock().unwrap().set_up_remote(metrics_socket, metrics_host);
  let metrics_guard = ProxyMetrics::run();
  METRICS.lock().unwrap().gauge("TEST", 42);

  let (sender, rec) = channel::<network::ServerMessage>();
  let (tx, jg) = network::http::start_listener("127.0.0.1:8080".parse().unwrap(), 500, 12000, sender);

  let http_front = messages::HttpFront { app_id: String::from("app_1"), hostname: String::from("lolcatho.st:8080"), path_begin: String::from("/"), port: 8080 };
  let http_instance = messages::Instance { app_id: String::from("app_1"), ip_address: String::from("127.0.0.1"), port: 1026 };
  tx.send(network::ProxyOrder::Command(String::from("ID_ABCD"), messages::Command::AddHttpFront(http_front)));
  tx.send(network::ProxyOrder::Command(String::from("ID_EFGH"), messages::Command::AddInstance(http_instance)));
  println!("HTTP -> {:?}", rec.recv().unwrap());
  println!("HTTP -> {:?}", rec.recv().unwrap());

  let (sender2, rec2) = channel::<network::ServerMessage>();

  let options = ssl::SSL_OP_CIPHER_SERVER_PREFERENCE | ssl::SSL_OP_NO_COMPRESSION |
               ssl::SSL_OP_NO_TICKET | ssl::SSL_OP_NO_SSLV2 |
               ssl::SSL_OP_NO_SSLV3 | ssl::SSL_OP_NO_TLSV1;
  let cipher_list = String::from("ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:\
                              ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:\
                              ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:\
                              DHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES256-GCM-SHA384:\
                              ECDHE-ECDSA-AES128-SHA256:ECDHE-RSA-AES128-SHA256:\
                              ECDHE-ECDSA-AES128-SHA:ECDHE-RSA-AES256-SHA384:\
                              ECDHE-RSA-AES128-SHA:ECDHE-ECDSA-AES256-SHA384:\
                              ECDHE-ECDSA-AES256-SHA:ECDHE-RSA-AES256-SHA:DHE-RSA-AES128-SHA256:\
                              DHE-RSA-AES128-SHA:DHE-RSA-AES256-SHA256:DHE-RSA-AES256-SHA:\
                              ECDHE-ECDSA-DES-CBC3-SHA:ECDHE-RSA-DES-CBC3-SHA:\
                              EDH-RSA-DES-CBC3-SHA:AES128-GCM-SHA256:AES256-GCM-SHA384:\
                              AES128-SHA256:AES256-SHA256:AES128-SHA:AES256-SHA:DES-CBC3-SHA:\
                              !DSS");

  let (tx2, jg2) = network::tls::start_listener("127.0.0.1:8443".parse().unwrap(), 500, 12000, Some((options, cipher_list)), sender2);
  let tls_front = messages::TlsFront { app_id: String::from("app_1"), hostname: String::from("lolcatho.st"), path_begin: String::from("/"), port: 8443, cert_path: String::from("assets/certificate.pem"), key_path: String::from("assets/key.pem") };
  tx2.send(network::ProxyOrder::Command(String::from("ID_IJKL"), messages::Command::AddTlsFront(tls_front)));
  let tls_instance = messages::Instance { app_id: String::from("app_1"), ip_address: String::from("127.0.0.1"), port: 1026 };
  tx2.send(network::ProxyOrder::Command(String::from("ID_MNOP"), messages::Command::AddInstance(tls_instance)));

  let tls_front2 = messages::TlsFront { app_id: String::from("app_2"), hostname: String::from("test.local"), path_begin: String::from("/"), port: 8443, cert_path: String::from("assets/cert_test.pem"), key_path: String::from("assets/key_test.pem") };
  tx2.send(network::ProxyOrder::Command(String::from("ID_QRST"), messages::Command::AddTlsFront(tls_front2)));
  let tls_instance2 = messages::Instance { app_id: String::from("app_2"), ip_address: String::from("127.0.0.1"), port: 1026 };
  tx2.send(network::ProxyOrder::Command(String::from("ID_UVWX"), messages::Command::AddInstance(tls_instance2)));

  println!("TLS -> {:?}", rec2.recv().unwrap());
  println!("TLS -> {:?}", rec2.recv().unwrap());
  println!("TLS -> {:?}", rec2.recv().unwrap());
  println!("TLS -> {:?}", rec2.recv().unwrap());

  let _ = jg.join();
  info!("good bye");
}

