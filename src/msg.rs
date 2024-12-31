//! User space messages.
//!
//! To setup a [`MsgListener`] use [`msg_listener`]. It returns the listener as
//! well as a [`MsgToken`], which can be used in [`try_send_msg`] and
//! [`send_msg`] to send a message to the created `MsgListener`.
