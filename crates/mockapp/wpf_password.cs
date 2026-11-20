// The end-to-end suite's WPF password window (crates/verbatim-e2e/src/
// scenarios/password.rs): a window titled by its first argument holding a
// "User name" text box with the text of its third argument and, below it,
// a "Password" PasswordBox holding its fourth. Once its content has been
// rendered it puts the focus on the text box and writes the file its
// second argument names, empty, and it exits when the window is closed.
// mockapp's build script compiles it with the .NET Framework's own
// compiler, so the window appears without a script host's start.

using System;
using System.IO;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;

static class WpfPassword
{
    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length != 4)
        {
            Console.Error.WriteLine("usage: wpf_password <title> <ready file> <user name> <password>");
            return 2;
        }
        string ready = args[1];
        var window = new Window();
        window.Title = args[0];
        window.Width = 600;
        window.Height = 250;
        window.WindowStartupLocation = WindowStartupLocation.CenterScreen;
        var panel = new StackPanel();
        var user = new TextBox();
        AutomationProperties.SetName(user, "User name");
        user.Text = args[2];
        user.Margin = new Thickness(20);
        var pass = new PasswordBox();
        AutomationProperties.SetName(pass, "Password");
        pass.Password = args[3];
        pass.Margin = new Thickness(20);
        panel.Children.Add(user);
        panel.Children.Add(pass);
        window.Content = panel;
        window.ContentRendered += (sender, e) =>
        {
            user.Focus();
            File.WriteAllText(ready, "");
        };
        window.ShowDialog();
        return 0;
    }
}
