use super::*;

pub(super) struct Captcha;
impl Signer for Captcha {
    fn profile(&self, config: &BrowserlessTaskConfig) -> SignerProfile {
        SignerProfile {
            id: "captcha",
            label: "通用图片验证码签到",
            browser: "browserless",
            sign_in_method: "ocr_captcha",
            browserless: BrowserlessTaskConfig {
                attendance_path: config.attendance_path.clone(),
                captcha_selector: config.captcha_selector.clone(),
                captcha_input_selector: config.captcha_input_selector.clone(),
                selector: config.selector.clone(),
                already_keywords: config.already_keywords.clone(),
                ..Default::default()
            },
        }
    }
}
