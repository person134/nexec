use clap::{Parser, Subcommand};

mod install;
mod detect;
mod config;
mod update;
mod entry;

#[derive(Parser)]
#[command(name = "hboot", version, about = "hboot boot manager installer and management tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install hboot to the ESP and register with firmware
    Install {
        /// Path to the ESP mount point (auto-detected if not specified)
        #[arg(long)]
        esp: Option<String>,
        /// Disk device for efibootmgr (e.g. /dev/nvme0n1, /dev/sda)
        #[arg(long)]
        disk: Option<String>,
        /// Partition number for efibootmgr
        #[arg(long)]
        part: Option<u32>,
        /// Path to a pre-built hboot-efi.efi (builds from source if not specified)
        #[arg(long)]
        efi: Option<String>,
        /// Skip bootloader build (use --efi instead)
        #[arg(long)]
        no_build: bool,
        /// Sign the EFI binary for Secure Boot (auto-detects MOK keys)
        #[arg(long)]
        sign: bool,
        /// Path to the Secure Boot private key (implies --sign)
        #[arg(long)]
        sb_key: Option<String>,
        /// Path to the Secure Boot certificate/der (implies --sign)
        #[arg(long)]
        sb_cert: Option<String>,
        /// Skip config file creation/overwrite (used by hboot update)
        #[arg(long)]
        no_config: bool,
    },
    /// Detect OS installations on the ESP
    Detect {
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Manage hboot configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Show installation status
    Status,
    /// Remove hboot from the ESP and UEFI boot entries
    Remove {
        /// Path to the ESP mount point (auto-detected if not specified)
        #[arg(long)]
        esp: Option<String>,
        /// Skip efibootmgr cleanup
        #[arg(long)]
        no_efi: bool,
        /// Remove config file as well
        #[arg(long)]
        all: bool,
        /// Also remove the hboot CLI binary (/usr/bin/hboot)
        #[arg(long)]
        self_remove: bool,
    },
    /// Update hboot to the latest release
    Update,
    /// Manage boot entries
    Entry {
        #[command(subcommand)]
        action: EntryAction,
    },
}

#[derive(Subcommand)]
enum EntryAction {
    /// List all boot entries
    List {
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Add a new boot entry
    Add {
        /// Entry name (used as filename)
        name: String,
        /// Path to the .efi binary on the ESP
        #[arg(long)]
        efi: String,
        /// Display title (defaults to name)
        #[arg(long)]
        title: Option<String>,
        /// Kernel command-line options
        #[arg(long)]
        options: Option<String>,
        /// Path to initramfs on the ESP
        #[arg(long)]
        initrd: Option<String>,
        /// Number of boot tries (for boot counting)
        #[arg(long)]
        tries: Option<u32>,
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Remove a boot entry
    Remove {
        /// Entry name to remove
        name: String,
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Edit a boot entry in your editor
    Edit {
        /// Entry name to edit
        name: String,
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Mark a boot entry as good (remove boot counter)
    MarkGood {
        /// Entry name to mark as good
        name: String,
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Set boot tries for an entry
    SetTries {
        /// Entry name
        name: String,
        /// Number of boot tries (0 = no counter)
        tries: u32,
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Generate a sample hboot.conf
    Init {
        /// Output path (default: ./hboot.conf)
        #[arg(long, default_value = "./hboot.conf")]
        output: String,
    },
    /// Set the default boot entry
    SetDefault {
        /// Name of the entry to set as default
        entry: String,
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Print detected entries in hboot.conf format
    Detect {
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
    /// Open the config file in your editor
    Edit {
        /// Path to the ESP mount point
        #[arg(long)]
        esp: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install { esp, disk, part, efi, no_build, sign, sb_key, sb_cert, no_config } => {
            install::install(install::InstallArgs { esp_path: esp, disk, part, efi_path: efi, no_build, sign, sb_key, sb_cert, no_config });
        }
        Commands::Detect { esp } => {
            detect::detect(esp);
        }
        Commands::Config { action } => match action {
            ConfigAction::Init { output } => {
                config::init(output);
            }
            ConfigAction::SetDefault { entry, esp } => {
                config::set_default(entry, esp);
            }
            ConfigAction::Detect { esp } => {
                config::detect(esp);
            }
            ConfigAction::Edit { esp } => {
                config::edit(esp);
            }
        },
        Commands::Status => {
            install::status();
        }
        Commands::Remove { esp, no_efi, all, self_remove } => {
            install::remove(esp, no_efi, all, self_remove);
        }
        Commands::Update => {
            update::update();
        }
        Commands::Entry { action } => match action {
            EntryAction::List { esp } => {
                entry::list(esp);
            }
            EntryAction::Add { name, efi, title, options, initrd, tries, esp } => {
                entry::add(name, esp, efi, title, options, initrd, tries);
            }
            EntryAction::Remove { name, esp } => {
                entry::remove(name, esp);
            }
            EntryAction::Edit { name, esp } => {
                entry::edit(name, esp);
            }
            EntryAction::MarkGood { name, esp } => {
                entry::mark_good(name, esp);
            }
            EntryAction::SetTries { name, tries, esp } => {
                entry::set_tries(name, tries, esp);
            }
        },
    }
}
