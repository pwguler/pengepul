/// Prefix that routes a model to the opencode provider, e.g. `opencode/glm-5.1`.
pub const OPENCODE_PREFIX: &str = "opencode/";

/// Strip the `opencode/` routing prefix to get the upstream model id.
#[must_use]
pub fn strip_opencode_prefix(model: &str) -> &str {
    model.strip_prefix(OPENCODE_PREFIX).unwrap_or(model)
}

#[cfg(test)]
mod tests {
    use super::strip_opencode_prefix;

    #[test]
    fn strips_prefix_for_upstream() {
        assert_eq!(strip_opencode_prefix("opencode/kimi-k2.6"), "kimi-k2.6");
        assert_eq!(strip_opencode_prefix("kimi-k2.6"), "kimi-k2.6");
    }
}
