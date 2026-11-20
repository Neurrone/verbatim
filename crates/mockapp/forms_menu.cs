// A Windows Forms window titled by its one argument, whose menu bar holds
// one menu item, "View", checked, for mockapp's remote_ops test of a
// Windows Forms menu item (tests/remote_ops.rs, FormsMenu). mockapp's
// build script compiles it with the .NET Framework's own compiler, so the
// window appears without a script host's start. It writes "shown" to its
// standard output once the window is shown, and exits when it is closed.

using System;
using System.Windows.Forms;

static class FormsMenu
{
    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length != 1)
        {
            Console.Error.WriteLine("usage: forms_menu <title>");
            return 2;
        }
        var form = new Form();
        form.Text = args[0];
        var menu = new MenuStrip();
        var view = new ToolStripMenuItem("View");
        view.Checked = true;
        menu.Items.Add(view);
        form.MainMenuStrip = menu;
        form.Controls.Add(menu);
        form.Shown += (sender, e) =>
        {
            Console.Out.WriteLine("shown");
            Console.Out.Flush();
        };
        form.ShowDialog();
        return 0;
    }
}
