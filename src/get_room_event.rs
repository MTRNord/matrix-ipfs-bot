// you may have to import this, make sure it's the same version as matrix-sdk has it bit me before
use ruma_api::ruma_api;
// these should be matrix_sdk::whatever...
use matrix_sdk::events::{collections::all::RoomEvent, EventJson};
use matrix_sdk::identifiers::{EventId, RoomId};

ruma_api! {
    metadata {
        description: "Get a single event based on roomId/eventId",
        method: GET,
        name: "get_room_event",
        path: "/_matrix/client/r0/rooms/:room_id/event/:event_id",
        rate_limited: false,
        requires_authentication: true,
    }

    request {
        /// The ID of the room the event is in.
        #[ruma_api(path)]
        pub room_id: RoomId,

        /// The ID of the event.
        #[ruma_api(path)]
        pub event_id: EventId,
    }

    response {
        /// Arbitrary JSON of the event body. Returns both room and state events.
        #[ruma_api(body)]
        pub event: EventJson<RoomEvent>,
    }

    // Not positive about this one but I think the import is right
    error: matrix_sdk_common::api::Error
}
