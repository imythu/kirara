use super::*;
pub(super) struct Nexus;
impl Signer for Nexus {
    fn profile(&self, _: &BrowserlessTaskConfig) -> SignerProfile {
        SignerProfile {
            id: "nexus",
            label: "打开页面签到",
            browser: "lightpanda",
            sign_in_method: "open_page",
            browserless: BrowserlessTaskConfig::default(),
        }
    }
}
