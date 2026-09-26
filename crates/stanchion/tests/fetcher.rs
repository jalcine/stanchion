//! `async fn` in a `#[lua_class]` trait, driven by tokio.
#![cfg(feature = "async")]

use stanchion_lua::mlua::{Lua, Result};
use stanchion_lua::{load_class, lua_class};

const SOURCE: &str = include_str!("lua/fetcher.lua");

#[lua_class]
pub trait Fetcher {
    fn new(prefix: String) -> Result<Self>;

    /// Becomes `fn fetch(&self, url: String) -> BoxFuture<'_, Result<String>>`.
    async fn fetch(&self, url: String) -> Result<String>;

    #[lua(optional)]
    async fn warm_up(&self) -> Result<Option<String>>;
}

/// `dyn Fetcher` must stay `Send + Sync` under the `send` feature.
#[cfg(feature = "send")]
const _: () = {
    const fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn Fetcher>();
};

fn lua_with_async_host() -> Result<Lua> {
    let lua = Lua::new();
    let fetch_body = lua.create_async_function(|_, url: String| async move {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        Ok(format!("<{url}>"))
    })?;
    lua.globals().set("fetch_body", fetch_body)?;
    Ok(lua)
}

#[tokio::test]
async fn awaits_a_lua_method_that_yields() -> Result<()> {
    let lua = lua_with_async_host()?;
    let class: FetcherClass = load_class(&lua, SOURCE, "fetcher.lua")?;
    let fetcher = class.new("body: ".to_string())?;

    assert_eq!(fetcher.fetch("/a".to_string()).await?, "body: </a>");
    assert_eq!(fetcher.warm_up().await?, None);
    Ok(())
}

#[tokio::test]
async fn async_methods_work_behind_a_trait_object() -> Result<()> {
    let lua = lua_with_async_host()?;
    let class: FetcherClass = load_class(&lua, SOURCE, "fetcher.lua")?;
    let fetcher: Box<dyn Fetcher> = Box::new(class.new("body: ".to_string())?);

    assert_eq!(fetcher.fetch("/b".to_string()).await?, "body: </b>");
    Ok(())
}
