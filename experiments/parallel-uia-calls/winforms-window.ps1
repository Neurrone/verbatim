# A Windows Forms form (.NET Framework, Windows PowerShell 5.1) for the
# Windows Forms target: a multi-line text box with a long text, a list box
# of 200 items and 20 buttons, on this thread, its UI thread.
# Run with: powershell -STA -File winforms-window.ps1
Add-Type -AssemblyName System.Windows.Forms
$f = New-Object System.Windows.Forms.Form
$f.Text = 'puia winforms'
$f.Width = 600; $f.Height = 600
$tb = New-Object System.Windows.Forms.TextBox
$tb.Multiline = $true; $tb.ScrollBars = 'Vertical'; $tb.Width = 560; $tb.Height = 100
$tb.Text = ((1..3000) | ForEach-Object { "Line $_ of the Windows Forms text box." }) -join "`r`n"
$f.Controls.Add($tb)
foreach ($i in 1..20) {
    $b = New-Object System.Windows.Forms.Button
    $b.Text = "Button $i"; $b.Top = 110 + 25 * [math]::Floor(($i - 1) / 4); $b.Left = 10 + 140 * (($i - 1) % 4); $b.Width = 130
    $f.Controls.Add($b)
}
$list = New-Object System.Windows.Forms.ListBox
$list.Top = 250; $list.Width = 560; $list.Height = 280
foreach ($i in 1..200) { [void]$list.Items.Add("Item $i") }
$f.Controls.Add($list)
[void]$f.ShowDialog()
