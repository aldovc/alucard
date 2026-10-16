#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    link_section = ".init_array"
)]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[used]
static INITIALIZE: extern "C" fn() = initialize;

extern "C" fn initialize() {
    if std::env::var_os("ALUCARD_CI_POLL_INTERVAL").is_none() {
        std::env::set_var("ALUCARD_CI_POLL_INTERVAL", "0");
    }
    for key in [
        "ALUCARD_PROVIDER",
        "ALUCARD_WORKER_PROVIDER",
        "ALUCARD_CI_FIX_PROVIDER",
        "ALUCARD_REVIEWER_PROVIDER",
        "ALUCARD_FEEDBACK_PROVIDER",
        "ALUCARD_WRAPUP_PROVIDER",
    ] {
        std::env::remove_var(key);
    }
}
