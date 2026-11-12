pub fn build() -> Result<openrouter_rs::OpenRouterClient, openrouter_rs::error::OpenRouterError> {
    openrouter_rs::OpenRouterClient::builder().api_key("k").build()
}
