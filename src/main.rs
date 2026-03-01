/// A simple Meshcore bot that waits for "ping" messages in channel #test
/// and responds with its current location and the number of hops that the
/// ping message took to reach the bot.
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use anyhow::Result;
use config::Config;
use meshcore_rs::EventPayload;
use meshcore_rs::EventType;
use meshcore_rs::MeshCore;
use meshcore_rs::MeshCoreEvent;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use tokio::time::sleep;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Maximum on-device channels to try. It is unclear how many channel slots
/// the devices typically have, I've seen numbers between 8 and 32. Erring
/// on the side of caution here.
const MAX_MESHCORE_CHANNELS: u8 = 8;

#[derive(Error, Debug)]
pub enum MeshcoreBotError {
    #[error(
        "failed to join channel. Try creating a channel manually using a normal Meshcore client app"
    )]
    JoinChannelError,
    #[error("configuration value error")]
    ConfigValueError(String),
}

/// Get the channel index of the provided public channel. If the channel
/// is not found, register the channel in the first available index.
async fn join_channel(device: &mut MeshCore, channel: &str) -> Result<u8> {
    info!("joining channel {}", channel);
    // Hashtag channel key as per https://github.com/meshcore-dev/MeshCore/blob/main/docs/companion_protocol.md#channel-management
    let key: [u8; 16] = Sha256::digest(channel)
        .into_iter()
        .take(16)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    // Start with 1, since 0 is typically the "Public" channel.
    let mut channel_idx = 1;
    let mut first_available_index = None;
    while channel_idx < MAX_MESHCORE_CHANNELS {
        let channel_info = device
            .commands()
            .lock()
            .await
            .get_channel(channel_idx)
            .await
            .with_context(|| format!("retrieving channel info at index {channel_idx}"))?;
        debug!("channel info at index {channel_idx}: {channel_info:?}");
        if channel_info.name == channel {
            debug!("found matching channel at index {channel_idx}");
            return Ok(channel_idx);
        }
        if channel_info.name.is_empty() && first_available_index.is_none() {
            first_available_index = Some(channel_idx);
        }
        channel_idx += 1;
    }
    // Having not found the channel, add it in the first available slot.
    if let Some(channel_idx) = first_available_index {
        device
            .commands()
            .lock()
            .await
            .set_channel(channel_idx, channel, &key)
            .await
            .with_context(|| {
                format!("CMD_SET_CHANNEL for channel index {channel_idx} and channel {channel}")
            })?;
        debug!("created new channel at index {channel_idx}");
        return Ok(channel_idx);
    }
    Err(MeshcoreBotError::JoinChannelError.into())
}

/// Check if the incoming message is a channel message on the provided
/// test channel, or a private message. In either case, respond with a
/// hop count and location if provided.
async fn handle_message(
    device: &mut MeshCore,
    msg_event: MeshCoreEvent,
    test_channel: u8,
    location_info: &Option<String>,
    ping_hop_limit: &Option<i64>,
) {
    match (msg_event.event_type, msg_event.payload) {
        (EventType::ContactMsgRecv, EventPayload::ContactMessage(msg)) => {
            debug!("contact msg: {:?}", msg);
            warn!("private messages are not yet supported");
        }
        (EventType::ChannelMsgRecv, EventPayload::ChannelMessage(msg)) => {
            debug!("channel msg: {:?}", msg);
            let (sender, msg_text) = match msg.text.split_once(": ") {
                Some((s, t)) => (Some(s), t),
                None => (None, ""),
            };
            let sender = match sender {
                Some(s) => s,
                None => {
                    info!("channel message did not contain sender: {}", msg.text);
                    return;
                }
            };
            if !msg_text.eq_ignore_ascii_case("ping") || msg.channel_idx != test_channel {
                return;
            }
            if let Some(hop_limit) = ping_hop_limit
                && msg.path_len as i64 > *hop_limit
            {
                info!(
                    "ping request exceeded MESHCOREBOT_PING_HOP_LIMIT ({} > {})",
                    msg.path_len, hop_limit
                );
                return;
            }
            info!("received ping from {}, replying", sender);
            if let Err(e) = device
                .commands()
                .lock()
                .await
                .send_channel_msg(
                    test_channel,
                    &format!(
                        "@[{}]: QSL ({} hop{}) {}",
                        sender,
                        msg.path_len,
                        match msg.path_len {
                            1 => "",
                            _ => "s",
                        },
                        match location_info {
                            Some(i) => format!("QTH {}", i),
                            None => "".into(),
                        }
                    ),
                    None,
                )
                .await
            {
                error!("failed to send message: {:?}", e);
            }
        }
        _ => {}
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_env("MESHCOREBOT_LOG"))
        .init();
    info!("meshcorebot starting up");

    let settings = Config::builder()
        .add_source(config::Environment::with_prefix("MESHCOREBOT"))
        .build()?;

    let device_name = settings
        .get_string("device")
        .with_context(|| "provide a device name using environment variable MESHCOREBOT_DEVICE")?;
    let channel_name = settings
        .get_string("channel")
        .with_context(|| "provide a channel name using environment variable MESHCOREBOT_CHANNEL")?;
    let location_info = settings.get_string("location").ok();
    let flood_scope = settings.get_string("flood_scope").ok();
    let ping_hop_limit = settings.get_int("ping_hop_limit").ok();

    if !device_name.starts_with("MeshCore-") {
        warn!(
            "device name provided was {device_name}, typically device names contain the prefix MeshCore- (e.g. MeshCore-{device_name})"
        );
    }
    if !channel_name.starts_with("#") {
        warn!(
            "channel name provided was {channel_name}, typically channel names start with # (e.g. #{channel_name})"
        );
    }
    if flood_scope.is_none() {
        warn!("flood scope not set, consider setting it using MESHCOREBOT_FLOOD_SCOPE");
    }
    if let Some(limit) = &ping_hop_limit
        && *limit <= 0
    {
        return Err(MeshcoreBotError::ConfigValueError("ping_hop_limit".into())).with_context(
            || {
                format!(
                    "invalid value for MESHCOREBOT_PING_HOP_LIMIT: {} <= 0",
                    limit
                )
            },
        );
    }

    let mut device = MeshCore::ble_connect(&device_name)
        .await
        .with_context(|| format!("trying to connect to {device_name} over BLE"))?;
    let info = device
        .commands()
        .lock()
        .await
        .send_appstart()
        .await
        .with_context(|| "sending CMD_APP_START to BLE device")?;
    info!("connected to: {}", info.name);
    device.set_default_timeout(Duration::from_secs(10)).await;
    info!("setting time");
    if let Some(err) = device
        .commands()
        .lock()
        .await
        .set_time(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs() as u32,
        )
        .await
        .err()
    {
        warn!("encountered an error when trying to set device clock: {err:?}");
    }
    info!("setting flood scope");
    if let Some(err) = device
        .commands()
        .lock()
        .await
        .set_flood_scope(match &flood_scope {
            None => None,
            Some(scope) if scope == "*" => None,
            Some(scope) => Some(scope.as_str()),
        })
        .await
        .err()
    {
        warn!("encountered an error when trying to set flood scope: {err:?}");
    }
    let test_channel = join_channel(&mut device, &channel_name).await?;
    loop {
        let msg_result = device
            .commands()
            .lock()
            .await
            .get_msg()
            .await
            .with_context(|| "trying to retrieve the next unread message")?;
        match msg_result {
            Some(msg) => {
                handle_message(
                    &mut device,
                    msg,
                    test_channel,
                    &location_info,
                    &ping_hop_limit,
                )
                .await;
            }
            None => {
                sleep(Duration::from_millis(5000)).await;
            }
        }
    }
}
