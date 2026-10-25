//! YouTube on Pointman: a channel's videos uploaded and their details changed through the YouTube Data API v3
//! (`upload`, `channel`), the channel signed in from the apps with Google's device flow (`signin`), and the calls to
//! Google (`google`). The commands' params are proto/youtube/v1/youtube.proto's (`params`).

pub mod channel;
pub mod google;
pub mod params;
pub mod signin;
pub mod upload;
pub mod words;

/// The vault item with the Google OAuth client: every other `youtube.<label>` is a channel's sign-in.
pub const CLIENT_ITEM: &str = "youtube.client";
