// A basic client example.

use anyhow::{Context, Result};
use clap::Clap;
use domain::base::{Dname as DnameO, Message, MessageBuilder, ParsedDname, Rtype};
use domain::rdata::AllRecordData;
use log::trace;
use odoh_rs::*;
use rand::rngs::StdRng;
use rand::SeedableRng;
use reqwest::{Client, Url};

type Dname = DnameO<Vec<u8>>;

const WELL_KNOWN_CONF_PATH: &str = "/.well-known/odohconfigs";

#[derive(Clap, Debug)]
#[clap(version = "0.6")]
struct Opts {
    #[clap(short, long, default_value = "cloudflare.com")]
    domain: Dname,
    #[clap(name = "type", short, long, default_value = "AAAA")]
    rtype: Rtype,
    #[clap(short, long, default_value = "https://odoh.cloudflare-dns.com")]
    service: Url,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let opts: Opts = Opts::parse();

    trace!("Retrieving ODoH configs");
    let conf_url = opts
        .service
        .join(WELL_KNOWN_CONF_PATH)
        .context("Failed to compose well-known config path")?;
    let mut body = reqwest::get(conf_url)
        .await
        .context("failed to make request for config")?
        .bytes()
        .await
        .context("failed to get body")?;

    let configs: ObliviousDoHConfigs = parse(&mut body).context("invalid configs")?;
    let config = configs
        .into_iter()
        .next()
        .context("no available config")?
        .into();

    trace!("Creating DNS message");
    let mut msg = MessageBuilder::new_vec();
    msg.header_mut().set_rd(true);
    let mut msg = msg.question();
    msg.push((opts.domain, opts.rtype))
        .context("failed to push question")?;
    let msg = msg.finish();

    let mut rng = StdRng::from_entropy();

    trace!("Encrypting DNS message");
    let query = ObliviousDoHMessagePlaintext::new(&msg, 0);
    let (query_enc, cli_secret) =
        encrypt_query(&query, &config, &mut rng).context("failed to encrypt query")?;
    let query_body = compose(&query_enc)
        .context("failed to compose query body")?
        .freeze();

    trace!("Exchanging with server");
    let cli = Client::new();
    let mut resp_body = cli
        .post(opts.service.join("/dns-query")?)
        .header("content-type", ODOH_HTTP_HEADER)
        .header("accept", ODOH_HTTP_HEADER)
        .body(query_body)
        .send()
        .await
        .context("failed to query target server")?
        .bytes()
        .await
        .context("failed to get response body")?;

    trace!("Decrypting DNS message");
    let response_enc = parse(&mut resp_body).context("failed to parse response body")?;
    let response_dec = decrypt_response(&query, &response_enc, cli_secret)
        .context("failed to decrypt resopnse")?;

    let msg_bytes = response_dec.into_msg();
    let msg =
        Message::from_octets(msg_bytes.as_ref()).context("failed to parse response message")?;

    trace!("Printing DNS response");
    for (rr, _) in msg.for_slice().iter().filter_map(|r| r.ok()) {
        if rr.rtype() == Rtype::Opt {
            return Ok(());
        }

        if let Ok(Some(rr)) = rr.to_record::<AllRecordData<_, ParsedDname<_>>>() {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                rr.owner(),
                rr.ttl(),
                rr.class(),
                rr.rtype(),
                rr.data()
            )
        } else {
            println!(
                "{}\t{}\t{}\t{}",
                rr.owner(),
                rr.ttl(),
                rr.class(),
                rr.rtype()
            )
        }
    }

    Ok(())
}