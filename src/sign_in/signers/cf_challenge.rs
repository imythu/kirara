use super::*;
pub(super) struct CfChallenge;
impl Signer for CfChallenge {
    fn profile(&self, _: &BrowserlessTaskConfig) -> SignerProfile {
        SignerProfile {
            id: "nexus_cf_challenge",
            label: "CF 页面挑战签到",
            browser: "browserless",
            sign_in_method: "cloudflare",
            browserless: BrowserlessTaskConfig {
                cf_mode: "page".into(),
                ..Default::default()
            },
        }
    }
}
