use super::*;
/// OpenCD's popup form needs an awaited AJAX submission rather than a button click.
pub(super) struct OpenCd;
impl Signer for OpenCd {
    fn profile(&self, _: &BrowserlessTaskConfig) -> SignerProfile {
        let mut profile = captcha::Captcha.profile(&BrowserlessTaskConfig {
            attendance_path: "/plugin_sign-in.php".into(),
            captcha_selector: "#frmSignin img".into(),
            captcha_input_selector: "#imagestring".into(),
            selector: "#ok".into(),
            ..Default::default()
        });
        profile.id = "opencd";
        profile.label = "皇后 OpenCD 签到";
        profile.browserless.submit_method = "ajax".into();
        profile.browserless.result_rules = ["success", "false"]
            .into_iter()
            .map(|value| SignInResultRule {
                outcome: "success".into(),
                kind: "json".into(),
                selector: String::new(),
                field: "/state".into(),
                value: value.into(),
                value_type: "string".into(),
            })
            .collect();
        profile
    }
}
