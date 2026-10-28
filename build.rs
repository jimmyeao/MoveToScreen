fn main() {
    embed_resource::compile("jumpscreen.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
