iso_path       = "D:/isos/Win11.iso"
iso_sha256     = "0000000000000000000000000000000000000000000000000000000000000000"
image_name     = "Windows 11 Pro"
admin_username = "verbatim"
admin_password = "ChangeMe!Verbatim2026"
switch_name    = "Default Switch"

# Keep this on a volume with enough free space for the temporary VHDX.
temp_path = "artifacts/packer/tmp"

# Leave empty for the normal no-activation base image path.
# If a future ISO requires an install-only edition key, set it in an ignored
# local var file, not in committed configuration.
install_product_key = ""
