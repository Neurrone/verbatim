// The end-to-end suite's Windows Forms password form (crates/verbatim-e2e/
// src/scenarios/password.rs): a window titled by its first argument
// holding a "User name" text box with the text of its third argument and,
// below it, a "Password" text box with the system's password character
// holding its fourth. It writes the file its second argument names, empty,
// from the form's Shown event, once Windows Forms has finished showing it
// and put the focus on the first box, and exits when the window is closed.
// mockapp's build script compiles it with the .NET Framework's own
// compiler, so the window appears without a script host's start.

using System;
using System.Drawing;
using System.IO;
using System.Windows.Forms;

static class FormsPassword
{
    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length != 4)
        {
            Console.Error.WriteLine("usage: forms_password <title> <ready file> <user name> <password>");
            return 2;
        }
        string ready = args[1];
        Application.EnableVisualStyles();
        var form = new Form();
        form.Text = args[0];
        form.ClientSize = new Size(600, 200);
        form.StartPosition = FormStartPosition.CenterScreen;
        var user = new TextBox();
        user.AccessibleName = "User name";
        user.Text = args[2];
        user.Location = new Point(20, 20);
        user.Width = 400;
        var pass = new TextBox();
        pass.AccessibleName = "Password";
        pass.UseSystemPasswordChar = true;
        pass.Text = args[3];
        pass.Location = new Point(20, 80);
        pass.Width = 400;
        form.Controls.Add(user);
        form.Controls.Add(pass);
        form.Shown += (sender, e) => File.WriteAllText(ready, "");
        form.ShowDialog();
        return 0;
    }
}
