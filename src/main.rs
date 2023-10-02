use anyhow::{anyhow, Result};
use clap::Parser;
use futures::StreamExt;
use once_cell::sync::Lazy;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

static DEFAULT_TARGET: Lazy<String> =
    Lazy::new(|| format!("{}/usr/brave", std::env::var("HOME").unwrap_or_default(),));

#[derive(Parser, Debug)]
#[clap()]
struct Args {
    /// Target installation directory.
    #[structopt(long, short, default_value_t = DEFAULT_TARGET.to_string())]
    target: String,

    /// Build suffix.
    #[structopt(long, short, default_value = "-linux-amd64.zip")]
    suffix: String,
}

struct Release {
    name: String,
    url: String,
}

async fn get_latest_release(args: &Args) -> Result<Release> {
    let octocrab = octocrab::instance();
    let page = octocrab
        .repos("brave", "brave-browser")
        .releases()
        .list()
        .per_page(100)
        .send()
        .await?;
    for release in page {
        if let Some(ref name) = release.name {
            if name.starts_with("Release") {
                for asset in release.assets {
                    if asset.name.ends_with(&args.suffix) {
                        return Ok(Release {
                            name: name.trim().into(),
                            url: asset.browser_download_url.into(),
                        });
                    }
                }
            }
        }
    }
    Err(anyhow!("No Release Found"))
}

fn get_installed_version(args: &Args) -> Result<String> {
    match fs::read_to_string(format!("{}/version", args.target)) {
        Ok(contents) => Ok(contents),
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(String::new())
            } else {
                Err(err.into())
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let installed_version = get_installed_version(&args)?;
    let latest_release = get_latest_release(&args).await?;
    if installed_version == latest_release.name {
        println!("No updates, already current: {}", latest_release.name);
    } else {
        println!(
            "Upgrading from {} to {}",
            installed_version, latest_release.name
        );

        let mut tmp_file = tokio::fs::File::from(tempfile::tempfile()?);
        let mut byte_stream = reqwest::get(&latest_release.url).await?.bytes_stream();
        while let Some(item) = byte_stream.next().await {
            tokio::io::copy(&mut item?.as_ref(), &mut tmp_file).await?;
        }
        let tmp_file = tmp_file.into_std().await;
        let mut archive = zip::ZipArchive::new(tmp_file)?;
        let target_new = PathBuf::from(format!("{}.new", &args.target));
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };
            let outpath = target_new.join(outpath);

            if (*file.name()).ends_with('/') {
                fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p)?;
                    }
                }
                let mut outfile = fs::File::create(&outpath)?;
                io::copy(&mut file, &mut outfile)?;
            }

            if let Some(mode) = file.unix_mode() {
                fs::set_permissions(&outpath, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
        fs::write(target_new.join("version"), latest_release.name)?;
        fs::remove_dir_all(&args.target)?;
        fs::rename(&target_new, &args.target)?;
    }
    // TODO restart brave?
    Ok(())
}
