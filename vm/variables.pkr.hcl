variable "iso_path" {
  type        = string
  description = "Local Windows 11 ISO path supplied by the developer or self-hosted runner."
}

variable "iso_sha256" {
  type        = string
  description = "SHA-256 checksum for the local Windows 11 ISO."
}

variable "image_name" {
  type        = string
  default     = "Windows 11 Pro"
  description = "Windows image name selected from the multi-edition ISO."
}

variable "install_product_key" {
  type        = string
  default     = ""
  description = "Optional install-only edition key. Leave empty for the normal no-activation base image path."
  sensitive   = true
}

variable "vm_name_prefix" {
  type        = string
  default     = "verbatim-win11"
  description = "Prefix for temporary Packer build VMs."
}

variable "computer_name" {
  type        = string
  default     = "VERBATIMLAB"
  description = "Computer name assigned during unattended setup."
}

variable "admin_username" {
  type        = string
  default     = "verbatim"
  description = "Local administrator account created for automation."
  sensitive   = true
}

variable "admin_password" {
  type        = string
  description = "Local administrator password supplied outside git."
  sensitive   = true
}

variable "cpus" {
  type        = number
  default     = 4
  description = "Virtual processor count."
}

variable "memory_mb" {
  type        = number
  default     = 16384
  description = "VM memory in MB."
}

variable "disk_size_mb" {
  type        = number
  default     = 40960
  description = "VHDX size in MB."
}

variable "switch_name" {
  type        = string
  default     = "Default Switch"
  description = "Hyper-V virtual switch used during provisioning."
}

variable "output_directory" {
  type        = string
  default     = "artifacts/packer/windows11"
  description = "Generated Packer output directory."
}

variable "temp_path" {
  type        = string
  default     = "artifacts/packer/tmp"
  description = "Temporary Hyper-V VM and VHDX working directory. Keep this on a volume with enough free space for the full build disk."
}

variable "headless" {
  type        = bool
  default     = false
  description = "Whether Packer should hide the VM console during build."
}

variable "provisioning_version" {
  type        = string
  default     = "phase0-base-v1"
  description = "Version string written into C:\\VerbatimLab\\image.json."
}
