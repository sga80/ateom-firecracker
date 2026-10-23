use anyhow::Context;
use arcbox_ext4::Formatter;
use std::fs;
use std::fs::File;
use std::path::Path;
use uuid::Uuid;

pub fn docker_lower_ext4_block_device(path_to_flattened_docker_image: &str) -> anyhow::Result<(String)> {
    // takes the path on host and converts into a tar and then converts into a ext4 block device
    // first tar with tar
    let out_path_str = String::from("/run/ateomfc/image/counter.ext4");
    let output_path = Path::new(&out_path_str);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tar_output_path = Path::new("/tmp/counter.tar");
    if let Some(parent) = tar_output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tar_file = File::create(tar_output_path)?;
    let mut tar = tar::Builder::new(tar_file);
    tar.follow_symlinks(false); // do not follow symlinks as these are docker images
    tar.append_dir_all("", path_to_flattened_docker_image)?;
    tar.finish()?;

    let tar_reader = File::open(tar_output_path)?;

    let mut formatter = Formatter::new(output_path, 4096, 128 * 1024 * 1024)
        .context("creating ext4 formatter")?;
    formatter
        .unpack_tar(tar_reader)
        .context("unpacking tar into ext4 image")?;
    formatter.close().context("closing ext4 formatter")?;

    println!("wrote ext4 image to {}", output_path.display());
    Ok(out_path_str)
}

pub fn docker_upper_ext4_block_device(actor_id: Uuid) -> anyhow::Result<(String)> {
    let out_path_str = format!("/run/ateomfc/{actor_id}/upper.ext4");
    let output_path = Path::new(&out_path_str);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let formatter = Formatter::new(output_path, 4096, 128 * 1024 * 1024)
        .context("creating ext4 formatter")?;
    formatter.close().context("closing ext4 formatter")?;
    Ok(out_path_str)
}