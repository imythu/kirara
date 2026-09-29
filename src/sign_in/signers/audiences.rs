use super::*;

pub(super) struct Audiences;

impl Signer for Audiences {
    fn profile(&self, config: &BrowserlessTaskConfig) -> SignerProfile {
        let browserless = if config.manual_override {
            let mut manual = config.clone();
            let default_submit_selector = manual.selector == DEFAULT_BROWSERLESS_SELECTOR
                || manual.selector == "form[action*='attendance.php'] input[type='submit']";
            if manual.cf_mode == BROWSERLESS_CF_MODE_TURNSTILE
                && manual.submit_method == "click"
                && default_submit_selector
            {
                manual.selector = "#attendance-form".into();
                manual.submit_method = "form".into();
            }
            manual
        } else {
            BrowserlessTaskConfig {
                cf_mode: BROWSERLESS_CF_MODE_TURNSTILE.into(),
                selector: "#attendance-form".into(),
                submit_method: "form".into(),
                ..Default::default()
            }
        };
        SignerProfile {
            id: "audiences_cf_turnstile",
            label: "Audiences CF Turnstile",
            browser: "browserless",
            sign_in_method: "cloudflare",
            browserless,
        }
    }
}
