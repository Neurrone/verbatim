# A WPF window (.NET Framework, Windows PowerShell 5.1) for the WPF target:
# a text box holding a long text, a list of 200 items and 20 buttons, on
# this thread, its UI thread. Run with: powershell -STA -File wpf-window.ps1
Add-Type -AssemblyName PresentationFramework
$w = New-Object System.Windows.Window
$w.Title = 'puia wpf'
$w.Width = 600; $w.Height = 500
$panel = New-Object System.Windows.Controls.StackPanel
$tb = New-Object System.Windows.Controls.TextBox
$tb.AcceptsReturn = $true; $tb.Height = 100
$tb.Text = ((1..3000) | ForEach-Object { "Line $_ of the WPF text box." }) -join "`r`n"
[void]$panel.Children.Add($tb)
foreach ($i in 1..20) { $b = New-Object System.Windows.Controls.Button; $b.Content = "Button $i"; [void]$panel.Children.Add($b) }
$list = New-Object System.Windows.Controls.ListBox
$list.Height = 150
foreach ($i in 1..200) { [void]$list.Items.Add("Item $i") }
[void]$panel.Children.Add($list)
$w.Content = $panel
[void]$w.ShowDialog()
