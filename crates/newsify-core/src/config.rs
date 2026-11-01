//! Runtime configuration, read by the host (the Worker reads it from `[vars]`
//! and secrets) and handed to the service.

#[derive(Debug, Clone)]
pub struct Config {
    /// Brand shown in e-mail copy, e.g. "Chxta Inc".
    pub brand_name: String,
    /// `From` header, e.g. `Chxta Inc <news@example.com>`. Must be a sender
    /// verified with the e-mail provider.
    pub from: String,
    /// Public base URL of this API (no trailing slash). Used to build the
    /// confirm and unsubscribe links in e-mails.
    pub public_base_url: String,
    /// Public base URL of the website (no trailing slash). Browsers are
    /// redirected here after confirming a subscription.
    pub site_url: String,
    /// Where contact-form messages are e-mailed.
    pub contact_to: String,
    /// Secret for signing unsubscribe links and hashing IP addresses.
    pub signing_secret: String,
    /// E-mails per provider call. The provider allows at most 100.
    pub batch_size: usize,
    /// A delivery that fails this many times is given up on.
    pub max_attempts: u32,
    /// Do not send another confirmation e-mail to the same pending address
    /// within this window.
    pub confirm_cooldown_secs: i64,
    /// A confirmation link stops working after this long.
    pub confirm_ttl_secs: i64,
    /// Subscribe requests allowed per IP address per hour.
    pub subscribe_per_hour: u32,
    /// Contact messages allowed per IP address per hour.
    pub contact_per_hour: u32,
}

impl Config {
    /// Sensible defaults for everything that is not site specific.
    pub fn new(
        brand_name: impl Into<String>,
        from: impl Into<String>,
        public_base_url: impl Into<String>,
        site_url: impl Into<String>,
        contact_to: impl Into<String>,
        signing_secret: impl Into<String>,
    ) -> Self {
        Self {
            brand_name: brand_name.into(),
            from: from.into(),
            public_base_url: public_base_url.into().trim_end_matches('/').to_string(),
            site_url: site_url.into().trim_end_matches('/').to_string(),
            contact_to: contact_to.into(),
            signing_secret: signing_secret.into(),
            batch_size: 50,
            max_attempts: 5,
            confirm_cooldown_secs: 5 * 60,
            confirm_ttl_secs: 7 * 24 * 3600,
            subscribe_per_hour: 5,
            contact_per_hour: 3,
        }
    }
}
