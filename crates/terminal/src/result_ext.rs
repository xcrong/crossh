//! Minimal `Result` helper vendored from Zed's `gpui_util::ResultExt`.
//!
//! Only [`ResultExt::log_err`] — the single method this fork and its view
//! layer use. Crossh keeps no Zed dependency besides `gpui`/`gpui_platform`.

pub trait ResultExt<E> {
    type Ok;

    fn log_err(self) -> Option<Self::Ok>;
}

impl<T, E> ResultExt<E> for Result<T, E>
where
    E: std::fmt::Display,
{
    type Ok = T;

    #[track_caller]
    fn log_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log::error!("{}: {error}", std::panic::Location::caller());
                None
            }
        }
    }
}
