# Scratch Windows Forms window for the by-hand captures, in the form of the
# text_box_password scenario's script (crates/verbatim-e2e/src/scenarios/
# password.rs): -Kind listview makes a ListView in details view, listbox a
# ListBox, combobox a drop-down ComboBox, each holding Alpha, Beta and Gamma
# with the first item focused and selected; the form is titled -Title and
# writes the file -Ready from its Shown event.
param(
    [Parameter(Mandatory)] [ValidateSet('listview', 'listbox', 'combobox')] [string] $Kind,
    [Parameter(Mandatory)] [string] $Title,
    [Parameter(Mandatory)] [string] $Ready
)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
$form = New-Object System.Windows.Forms.Form
$form.Text = $Title
$form.ClientSize = New-Object System.Drawing.Size(600, 300)
$form.StartPosition = 'CenterScreen'
$names = 'Alpha', 'Beta', 'Gamma'
switch ($Kind) {
    'listview' {
        $control = New-Object System.Windows.Forms.ListView
        $control.View = 'Details'
        $control.FullRowSelect = $true
        $control.HideSelection = $false
        [void]$control.Columns.Add('Name', 200)
        foreach ($name in $names) { [void]$control.Items.Add($name) }
        $control.Items[0].Selected = $true
        $control.Items[0].Focused = $true
    }
    'listbox' {
        $control = New-Object System.Windows.Forms.ListBox
        foreach ($name in $names) { [void]$control.Items.Add($name) }
        $control.SelectedIndex = 0
    }
    'combobox' {
        $control = New-Object System.Windows.Forms.ComboBox
        $control.DropDownStyle = 'DropDownList'
        foreach ($name in $names) { [void]$control.Items.Add($name) }
        $control.SelectedIndex = 0
    }
}
$control.AccessibleName = 'Letters'
$control.Location = New-Object System.Drawing.Point(20, 20)
$control.Size = New-Object System.Drawing.Size(400, 200)
$form.Controls.Add($control)
$form.Add_Shown({ [void]$control.Focus(); [IO.File]::WriteAllText($Ready, '') })
[void]$form.ShowDialog()
