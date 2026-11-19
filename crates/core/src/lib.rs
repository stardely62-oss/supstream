//! Ядро Supstream: подключения к площадкам, шина событий, локальное API.

pub mod analysis;
pub mod app;
pub mod bus;
pub mod donationalerts;
pub mod events;
pub mod music;
pub mod server;
pub mod store;
pub mod twitch;

pub use app::{Running, StartOptions, start};
