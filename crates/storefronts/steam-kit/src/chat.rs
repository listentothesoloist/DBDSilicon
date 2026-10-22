//! Steam friends chat as it reaches the account's UI session. Unlike the
//! friends list these are events, not snapshots: a bounded ring keeps the
//! latest ones under increasing sequence numbers, and a consumer that falls
//! behind the ring learns it missed some and reloads the history instead.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

use prost::Message;
use tokio::sync::watch;

use crate::proto;

pub const INCOMING_MESSAGE: &str = "FriendMessagesClient.IncomingMessage#1";
pub const ACK_ECHO: &str = "FriendMessagesClient.NotifyAckMessageEcho#1";
/// EChatEntryType values (SteamKit enums.steamd).
pub const CHAT_MESSAGE: i32 = 1;
pub const TYPING: i32 = 2;
const RING: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub friend: u64,
    /// Sent by this account, from Obelisk or another of its sessions.
    pub from_me: bool,
    pub entry_type: i32,
    /// BBCode, as Steam stores it.
    pub message: String,
    pub plain: String,
    pub timestamp: u32,
    pub ordinal: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatEvent {
    Message(ChatMessage),
    Typing {
        friend: u64,
    },
    /// This account read the conversation up to `timestamp` in another session.
    Read {
        friend: u64,
        timestamp: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequencedChatEvent {
    pub sequence: u64,
    pub event: ChatEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatEvents {
    pub events: Vec<SequencedChatEvent>,
    pub last_sequence: u64,
    /// Events after the consumer's sequence were lost; history must be reloaded.
    pub gap: bool,
    pub connected: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cursor {
    last: u64,
    connected: bool,
}

struct Ring {
    events: VecDeque<SequencedChatEvent>,
    cursor: Cursor,
}

/// Fed by the CM reader with FriendMessagesClient notifications; public so
/// debug fixtures exercise the same decoding and observation path.
#[derive(Clone)]
pub struct ChatEventSource {
    ring: Arc<Mutex<Ring>>,
    updates: watch::Sender<Cursor>,
}

impl Default for ChatEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatEventSource {
    pub fn new() -> Self {
        let cursor = Cursor {
            last: 0,
            connected: true,
        };
        Self {
            ring: Arc::new(Mutex::new(Ring {
                events: VecDeque::with_capacity(RING),
                cursor,
            })),
            updates: watch::channel(cursor).0,
        }
    }

    /// Applies one service notification. Other services and malformed bodies
    /// are ignored: chat is optional and must never fail the connection.
    pub fn receive(&self, method: &str, body: &[u8]) {
        let event = match method {
            INCOMING_MESSAGE => {
                let Ok(incoming) = proto::IncomingMessage::decode(body) else {
                    return;
                };
                if incoming.chat_entry_type == TYPING {
                    // The account's own typing elsewhere is not the friend's.
                    if incoming.local_echo {
                        return;
                    }
                    ChatEvent::Typing {
                        friend: incoming.steamid_friend,
                    }
                } else if incoming.message.is_empty() {
                    return;
                } else {
                    ChatEvent::Message(ChatMessage {
                        friend: incoming.steamid_friend,
                        from_me: incoming.local_echo,
                        entry_type: incoming.chat_entry_type,
                        message: incoming.message,
                        plain: incoming.message_no_bbcode,
                        timestamp: incoming.rtime32_server_timestamp,
                        ordinal: incoming.ordinal,
                    })
                }
            }
            ACK_ECHO => {
                let Ok(ack) = proto::AckMessage::decode(body) else {
                    return;
                };
                ChatEvent::Read {
                    friend: ack.steamid_partner,
                    timestamp: ack.timestamp,
                }
            }
            _ => return,
        };
        self.push(event);
    }

    fn push(&self, event: ChatEvent) {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        if !ring.cursor.connected {
            return;
        }
        ring.cursor.last += 1;
        if ring.events.len() == RING {
            ring.events.pop_front();
        }
        let sequence = ring.cursor.last;
        ring.events
            .push_back(SequencedChatEvent { sequence, event });
        self.updates.send_replace(ring.cursor);
    }

    /// Waits for events after `after_sequence`. A cursor from another source
    /// (beyond the last sequence) answers at once with a gap. Disconnection
    /// wakes every wait; dropping the future cancels it.
    pub async fn next(&self, after_sequence: u64) -> ChatEvents {
        let _ = obelisk_core::watch::next_matching(self.updates.subscribe(), |cursor| {
            cursor.last != after_sequence || !cursor.connected
        })
        .await;
        let ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        let last = ring.cursor.last;
        let first = ring.events.front().map_or(last + 1, |event| event.sequence);
        let stale = after_sequence > last;
        ChatEvents {
            events: ring
                .events
                .iter()
                .filter(|event| stale || event.sequence > after_sequence)
                .cloned()
                .collect(),
            last_sequence: last,
            gap: stale || after_sequence + 1 < first,
            connected: ring.cursor.connected,
        }
    }

    pub fn disconnect(&self) {
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        ring.cursor.connected = false;
        self.updates.send_replace(ring.cursor);
    }
}

/// Wire bodies for debug fixtures, so tests feed the production decoders.
#[cfg(debug_assertions)]
pub mod fixture {
    use prost::Message;

    use crate::proto;

    pub fn incoming_message(
        friend: u64,
        entry_type: i32,
        message: &str,
        timestamp: u32,
        ordinal: u32,
        local_echo: bool,
    ) -> Vec<u8> {
        proto::IncomingMessage {
            steamid_friend: friend,
            chat_entry_type: entry_type,
            message: message.into(),
            rtime32_server_timestamp: timestamp,
            ordinal,
            local_echo,
            message_no_bbcode: message.into(),
        }
        .encode_to_vec()
    }

    pub fn ack_echo(friend: u64, timestamp: u32) -> Vec<u8> {
        proto::AckMessage {
            steamid_partner: friend,
            timestamp,
        }
        .encode_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRIEND: u64 = 76_561_197_960_265_729;

    fn incoming(entry_type: i32, message: &str, ordinal: u32, local_echo: bool) -> Vec<u8> {
        proto::IncomingMessage {
            steamid_friend: FRIEND,
            chat_entry_type: entry_type,
            message: message.into(),
            rtime32_server_timestamp: 1_700_000_000,
            ordinal,
            local_echo,
            message_no_bbcode: message.replace("[b]", "").replace("[/b]", ""),
        }
        .encode_to_vec()
    }

    #[test]
    fn independent_wire_fixture_decodes_an_incoming_message() {
        // steamid_friend fixed64 1, chat_entry_type 2, message 4,
        // rtime32_server_timestamp fixed32 5, ordinal 6, local_echo 7, message_no_bbcode 8.
        let mut body = vec![0x09];
        body.extend_from_slice(&FRIEND.to_le_bytes());
        body.extend_from_slice(&[0x10, 0x01, 0x22, 0x02, b'h', b'i', 0x2d]);
        body.extend_from_slice(&1_700_000_000_u32.to_le_bytes());
        body.extend_from_slice(&[0x30, 0x03, 0x38, 0x01, 0x42, 0x02, b'h', b'i']);
        let source = ChatEventSource::new();
        source.receive(INCOMING_MESSAGE, &body);
        source.receive("ChatRoomClient.NotifyIncomingChatMessage#1", &body);
        source.receive(INCOMING_MESSAGE, &[0xff]);
        let events = source.events_now(0);
        assert_eq!(
            events.events,
            vec![SequencedChatEvent {
                sequence: 1,
                event: ChatEvent::Message(ChatMessage {
                    friend: FRIEND,
                    from_me: true,
                    entry_type: CHAT_MESSAGE,
                    message: "hi".into(),
                    plain: "hi".into(),
                    timestamp: 1_700_000_000,
                    ordinal: 3,
                }),
            }]
        );
    }

    #[test]
    fn typing_read_and_empty_entries() {
        let source = ChatEventSource::new();
        source.receive(INCOMING_MESSAGE, &incoming(TYPING, "", 0, false));
        // The account typing in another session, and an empty leave notice.
        source.receive(INCOMING_MESSAGE, &incoming(TYPING, "", 0, true));
        source.receive(INCOMING_MESSAGE, &incoming(6, "", 0, false));
        source.receive(
            ACK_ECHO,
            &proto::AckMessage {
                steamid_partner: FRIEND,
                timestamp: 42,
            }
            .encode_to_vec(),
        );
        let events: Vec<_> = source
            .events_now(0)
            .events
            .into_iter()
            .map(|event| event.event)
            .collect();
        assert_eq!(
            events,
            vec![
                ChatEvent::Typing { friend: FRIEND },
                ChatEvent::Read {
                    friend: FRIEND,
                    timestamp: 42
                },
            ]
        );
    }

    #[test]
    fn a_consumer_behind_the_ring_reads_a_gap() {
        let source = ChatEventSource::new();
        for ordinal in 0..RING as u32 + 10 {
            source.receive(
                INCOMING_MESSAGE,
                &incoming(CHAT_MESSAGE, "[b]x[/b]", ordinal, false),
            );
        }
        let behind = source.events_now(5);
        assert!(behind.gap);
        assert_eq!(behind.events.len(), RING);
        assert_eq!(behind.events[0].sequence, 11);
        assert_eq!(behind.last_sequence, RING as u64 + 10);
        // Exactly at the oldest kept event's predecessor is not a gap.
        let edge = source.events_now(10);
        assert!(!edge.gap);
        assert_eq!(edge.events.len(), RING);
        let caught_up = source.events_now(RING as u64 + 5);
        assert!(!caught_up.gap);
        assert_eq!(caught_up.events.len(), 5);
    }

    #[tokio::test]
    async fn waits_for_new_events_and_wakes_on_disconnect() {
        let source = ChatEventSource::new();
        let waiting = tokio::spawn({
            let source = source.clone();
            async move { source.next(0).await }
        });
        tokio::task::yield_now().await;
        source.receive(INCOMING_MESSAGE, &incoming(CHAT_MESSAGE, "hi", 1, false));
        let events = waiting.await.unwrap();
        assert_eq!(events.events.len(), 1);
        assert!(events.connected && !events.gap);

        let waiting = tokio::spawn({
            let source = source.clone();
            async move { source.next(1).await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        source.disconnect();
        let ended = waiting.await.unwrap();
        assert!(!ended.connected);
        assert!(ended.events.is_empty());
        // Nothing reaches a disconnected source.
        source.receive(INCOMING_MESSAGE, &incoming(CHAT_MESSAGE, "late", 2, false));
        assert_eq!(source.next(1).await.last_sequence, 1);
    }

    #[tokio::test]
    async fn a_cursor_from_another_session_answers_at_once_with_a_gap() {
        let source = ChatEventSource::new();
        source.receive(INCOMING_MESSAGE, &incoming(CHAT_MESSAGE, "hi", 1, false));
        let events = source.next(900).await;
        assert!(events.gap);
        assert_eq!(events.events.len(), 1);
        assert_eq!(events.last_sequence, 1);
    }

    impl ChatEventSource {
        /// `next` for a cursor known to have events after it.
        fn events_now(&self, after_sequence: u64) -> ChatEvents {
            futures_util::FutureExt::now_or_never(self.next(after_sequence))
                .expect("events are available")
        }
    }
}
