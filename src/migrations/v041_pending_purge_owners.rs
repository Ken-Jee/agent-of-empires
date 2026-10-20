use anyhow::Result;

pub fn run() -> Result<()> {
    let root = crate::session::get_app_dir()?;
    if !root
        .join(crate::session::purge_owners::FILE_NAME)
        .try_exists()?
    {
        crate::session::purge_owners::initialize(&root)?;
    }
    tracing::info!("Initialized durable pending purge ownership");
    Ok(())
}
