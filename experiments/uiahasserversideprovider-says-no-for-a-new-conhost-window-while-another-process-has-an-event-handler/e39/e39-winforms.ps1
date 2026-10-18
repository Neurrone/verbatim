# e39 experiment 3: a Windows Forms window like the e2e text_box
# scenario's (a multi-line TextBox), with a button and a list box beside it.
Add-Type -AssemblyName System.Windows.Forms
$form = New-Object System.Windows.Forms.Form
$form.Text = 'e39-winforms'
$box = New-Object System.Windows.Forms.TextBox
$box.Multiline = $true
$box.Text = "Apple`r`nBanana"
$box.SetBounds(10, 10, 200, 80)
$button = New-Object System.Windows.Forms.Button
$button.Text = 'Press'
$button.SetBounds(10, 100, 80, 30)
$list = New-Object System.Windows.Forms.ListBox
$list.Items.AddRange(@('one', 'two'))
$list.SetBounds(220, 10, 100, 80)
$form.Controls.AddRange(@($box, $button, $list))
[System.Windows.Forms.Application]::Run($form)
