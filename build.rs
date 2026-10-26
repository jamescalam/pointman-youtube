// The proto's messages as Rust types (buffa's), for the typed handlers: proto/youtube/v1/youtube.proto.
fn main() {
    pointman_extension::proto::build().expect("proto/youtube/v1/youtube.proto builds");
    // the client a release comes with (src/google.rs): built again when it changes
    println!("cargo:rerun-if-env-changed=YOUTUBE_DEFAULT_CLIENT");
}
