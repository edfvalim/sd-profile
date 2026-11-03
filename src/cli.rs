//! Command-line arguments.

use std::path::PathBuf;

use clap::Parser;

/// Profile SD and microSD cards: identity, speed, reader, partitions,
/// filesystems and mounts.
#[derive(Debug, Parser)]
#[command(version)]
pub struct Args {
    /// Disk image or card to profile. A disk image is a file such as a card
    /// backup made with `dd` or a Raspberry Pi OS `.img`; a card is its
    /// disk's device, like `/dev/disk6`. Without it, the card in this
    /// computer's card reader.
    pub target: Option<PathBuf>,

    /// Print the profile as JSON instead of the readable report.
    #[arg(long)]
    pub json: bool,
}

/// How the profile is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

impl Args {
    #[must_use]
    pub fn format(&self) -> Format {
        if self.json {
            Format::Json
        } else {
            Format::Human
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn definition_is_valid() {
        Args::command().debug_assert();
    }

    #[test]
    fn json_flag_selects_format() {
        let args = Args::parse_from(["sd-profile", "--json", "card.img"]);
        assert_eq!(args.format(), Format::Json);
        assert_eq!(args.target, Some(PathBuf::from("card.img")));
        assert_eq!(
            Args::parse_from(["sd-profile", "card.img"]).format(),
            Format::Human
        );
    }

    #[test]
    fn target_is_optional() {
        assert_eq!(Args::parse_from(["sd-profile"]).target, None);
    }
}
