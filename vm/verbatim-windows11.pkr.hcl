packer {
  required_version = ">= 1.15.0"

  required_plugins {
    hyperv = {
      source  = "github.com/hashicorp/hyperv"
      version = ">= 1.1.3"
    }
  }
}

locals {
  admin_username_xml      = replace(replace(replace(var.admin_username, "&", "&amp;"), "<", "&lt;"), ">", "&gt;")
  admin_password_xml      = replace(replace(replace(var.admin_password, "&", "&amp;"), "<", "&lt;"), ">", "&gt;")
  computer_name_xml       = replace(replace(replace(var.computer_name, "&", "&amp;"), "<", "&lt;"), ">", "&gt;")
  image_name_xml          = replace(replace(replace(var.image_name, "&", "&amp;"), "<", "&lt;"), ">", "&gt;")
  install_product_key_xml = replace(replace(replace(var.install_product_key, "&", "&amp;"), "<", "&lt;"), ">", "&gt;")

  autounattend_xml = templatefile("answer-files/Autounattend.xml.pkrtpl.hcl", {
    admin_username      = local.admin_username_xml
    admin_password      = local.admin_password_xml
    computer_name       = local.computer_name_xml
    image_name          = local.image_name_xml
    install_product_key = local.install_product_key_xml
  })
}

source "hyperv-iso" "windows11" {
  vm_name          = "${var.vm_name_prefix}-build"
  iso_url          = var.iso_path
  iso_checksum     = "sha256:${var.iso_sha256}"
  output_directory = var.output_directory
  temp_path        = var.temp_path

  generation           = 2
  enable_secure_boot   = true
  secure_boot_template = "MicrosoftWindows"
  enable_tpm           = true
  first_boot_device    = "DVD"
  boot_wait            = "1s"
  boot_command         = ["a<wait>a<wait>a<wait>a<wait>a<wait>a"]

  cpus      = var.cpus
  memory    = var.memory_mb
  disk_size = var.disk_size_mb

  switch_name = var.switch_name
  headless    = var.headless

  communicator   = "winrm"
  winrm_username = var.admin_username
  winrm_password = var.admin_password
  winrm_timeout  = "30m"

  cd_label = "VERBATIMLAB"
  cd_content = {
    "Autounattend.xml" = local.autounattend_xml
  }

  shutdown_command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"Stop-Computer -Force\""
  shutdown_timeout = "30m"
}

build {
  sources = ["source.hyperv-iso.windows11"]

  provisioner "powershell" {
    script = "${path.root}/scripts/Initialize-VerbatimBaseImage.ps1"
    environment_vars = [
      "VERBATIM_IMAGE_NAME=${var.image_name}",
      "VERBATIM_PROVISIONING_VERSION=${var.provisioning_version}",
    ]
  }

  # Set-DefaultAudioRenderDevice.ps1 is a standalone guest script that
  # `cargo xtask vm test --record` sends over PowerShell Direct before every
  # recording, to re-assert VB-CABLE as the default render device in case a
  # prior `cargo xtask vm connect` session left a stale default (see
  # xtask/src/vm/recording.rs). It travels in as its own "file" provisioner,
  # staged to C:\VerbatimLab\tools, rather than embedded as text inside
  # Initialize-VerbatimHarness.ps1, since Packer's "powershell" provisioner
  # uploads and runs exactly one script file with no access to siblings —
  # the destination directory already exists by this point
  # (Initialize-VerbatimBaseImage.ps1 creates it) precisely so this can run
  # before the harness provisioner.
  provisioner "file" {
    source      = "${path.root}/scripts/Set-DefaultAudioRenderDevice.ps1"
    destination = "C:/VerbatimLab/tools/Set-DefaultAudioRenderDevice.ps1"
  }

  # ffmpeg.exe and ffprobe.exe are deliberately NOT part of this image build.
  # cargo xtask vm deploy copies them into the guest at C:\VerbatimLab\tools
  # over PowerShell Direct (VMBus) alongside Verbatim's own binaries, from the
  # LFS-vendored vm/vendor/ffmpeg copy: that channel is fast and needs no
  # network reachability, unlike both an in-guest download (throttled or
  # stalled over Hyper-V's NAT) and a WinRM file provisioner (which hung
  # base64-streaming the ~100 MB binaries). See vm/vendor/ffmpeg/README.md and
  # docs/tooling.md's image-rebuild section.

  # The M2 harness provisioner (autologon, unattended-session settings,
  # display resolution, VB-CABLE audio, the VerbatimAgent scheduled task, and
  # its firewall rule): see that script's own header comment for
  # the full step list. Credentials are the same ones the source block
  # already uses for WinRM, reused rather than duplicated so there is
  # exactly one place that names the automation account.
  provisioner "powershell" {
    script = "${path.root}/scripts/Initialize-VerbatimHarness.ps1"
    environment_vars = [
      "VERBATIM_VM_USERNAME=${var.admin_username}",
      "VERBATIM_VM_PASSWORD=${var.admin_password}",
    ]
  }
}
