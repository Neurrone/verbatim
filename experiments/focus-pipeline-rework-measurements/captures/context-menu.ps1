# Scratch Windows Forms window for the focus pipeline rework's capture of
# a classic Win32 context menu (docs/design/focus-pipeline-rework.md,
# difference 17, the delayed pump), never merged. In the form of the
# text_box_password scenario's script: a form titled by -Title holding a
# "Notes" text box with the text "hello", whose ContextMenu (the classic
# Win32 menu, not a ContextMenuStrip) holds "First item", "Second item"
# and "Third item". Writes the file -Ready from its Shown event. Run it in
# Windows PowerShell 5.1.
param([string] $Title = 'Context menu capture', [string] $Ready = '')
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
$form = New-Object System.Windows.Forms.Form
$form.Text = $Title
$form.ClientSize = New-Object System.Drawing.Size(600, 200)
$form.StartPosition = 'CenterScreen'
$notes = New-Object System.Windows.Forms.TextBox
$notes.AccessibleName = 'Notes'
$notes.Text = 'hello'
$notes.Location = New-Object System.Drawing.Point(20, 20)
$notes.Width = 400
$menu = New-Object System.Windows.Forms.ContextMenu
foreach ($name in 'First item', 'Second item', 'Third item') { [void]$menu.MenuItems.Add($name) }
$notes.ContextMenu = $menu
$form.Controls.Add($notes)
if ($Ready -ne '') { $form.Add_Shown({ [IO.File]::WriteAllText($Ready, '') }) }
[void]$form.ShowDialog()
