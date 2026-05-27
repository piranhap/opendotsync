use crate::config::AppConfig;

pub fn current_profile(config: &AppConfig) -> &str {
    &config.profile
}
