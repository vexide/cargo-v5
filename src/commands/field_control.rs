use std::time::Duration;

use vex_v5_serial::{
    connection::{
        serial::{SerialConnection, SerialError},
        Connection,
    },
    packets::match_mode::{
        MatchMode, SetMatchModePacket, SetMatchModePayload, SetMatchModeReplyPacket,
    },
};

async fn set_match_mode(
    connection: &mut SerialConnection,
    match_mode: MatchMode,
) -> Result<(), SerialError> {
    connection
        .packet_handshake::<SetMatchModeReplyPacket>(
            Duration::from_millis(500),
            10,
            SetMatchModePacket::new(SetMatchModePayload {
                match_mode,
                match_time: 0,
            }),
        )
        .await?;
    Ok(())
}
