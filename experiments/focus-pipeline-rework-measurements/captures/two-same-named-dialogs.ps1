# Scratch Windows Forms window for the capture of a focus moving from a
# control in one child dialog into a second child dialog of the same role
# and name (two-same-named-dialogs.txt), never merged. A form titled by
# -Title holds two panels, each a child window of its own whose MSAA client
# object has role dialog and name "Remove" (AccessibleRole and
# AccessibleName), the first holding a "First" button and the second a
# "Second" button. Tab moves the focus from one button to the other. The
# first button has the focus when the form is shown; the Shown event writes
# the windows' handles to the file -Ready. Run it in Windows PowerShell 5.1.
param([string] $Title = 'Two dialogs capture', [string] $Ready = '')
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
$form = New-Object System.Windows.Forms.Form
$form.Text = $Title
$form.ClientSize = New-Object System.Drawing.Size(600, 200)
$form.StartPosition = 'CenterScreen'
$buttons = @()
$panels = @()
foreach ($i in 0, 1) {
    $panel = New-Object System.Windows.Forms.Panel
    $panel.AccessibleRole = [System.Windows.Forms.AccessibleRole]::Dialog
    $panel.AccessibleName = 'Remove'
    $panel.BorderStyle = 'FixedSingle'
    $panel.Location = New-Object System.Drawing.Point((20 + 290 * $i), 20)
    $panel.Size = New-Object System.Drawing.Size(270, 160)
    $panel.TabIndex = $i
    $button = New-Object System.Windows.Forms.Button
    $button.Text = ('First', 'Second')[$i]
    $button.Location = New-Object System.Drawing.Point(20, 20)
    $panel.Controls.Add($button)
    $form.Controls.Add($panel)
    $panels += $panel
    $buttons += $button
}
$form.Add_Shown({
    [void]$buttons[0].Focus()
    if ($Ready -ne '') {
        $lines = @("form $($form.Handle.ToInt64())")
        foreach ($i in 0, 1) {
            $lines += "panel$i $($panels[$i].Handle.ToInt64()) button$i $($buttons[$i].Handle.ToInt64())"
        }
        [IO.File]::WriteAllLines($Ready, $lines)
    }
})
[void]$form.ShowDialog()
