use super::*;
/// Reuse the five-field captcha signer with fixed site parameters.
pub(super) struct Baozi;
impl Signer for Baozi {
    fn profile(&self, _: &BrowserlessTaskConfig) -> SignerProfile {
        let mut profile = captcha::Captcha.profile(&BrowserlessTaskConfig {
            captcha_selector: "form[action='attendance.php'] img[alt='CAPTCHA']".into(),
            captcha_input_selector: "form[action='attendance.php'] input[name='imagestring']"
                .into(),
            selector: "form[action='attendance.php'] input[type='submit']".into(),
            already_keywords: "签到成功".into(),
            ..Default::default()
        });
        profile.id = "baozi";
        profile.label = "包子签到";
        profile
    }
}
