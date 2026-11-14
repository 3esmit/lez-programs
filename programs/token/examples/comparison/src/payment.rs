pub fn handle_message(input: &lee_core::program::ReceiveInput) -> lee_core::program::Response {
    crate::receipts::handle_message(input, false)
}
