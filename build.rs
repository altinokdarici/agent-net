fn main() {
    println!("cargo:rustc-env=TARGET={}", std::env::var("TARGET").unwrap());
    println!("cargo:rerun-if-env-changed=AGENT_NET_VERSION");
    println!("cargo:rerun-if-changed=.github/extensions/agent-net/extension.mjs");
}
