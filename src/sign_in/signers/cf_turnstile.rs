use super::*;
pub(super) struct CfTurnstile;
impl Signer for CfTurnstile {
    fn profile(&self, _: &BrowserlessTaskConfig) -> SignerProfile {
        SignerProfile {
            id: "nexus_cf_turnstile",
            label: "CF Turnstile 签到",
            browser: "browserless",
            sign_in_method: "cloudflare",
            browserless: BrowserlessTaskConfig {
                cf_mode: "turnstile".into(),
                selector: "form[action*='attendance.php'] input[type='submit']".into(),
                ..Default::default()
            },
        }
    }
}
