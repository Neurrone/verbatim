// The end-to-end suite's text box form (crates/verbatim-e2e/src/scenarios/
// text_box.rs): a Windows Forms window titled by its first argument, whose
// whole client area is one multi-line text box that does not wrap, showing
// thirteen and a half lines of its font at any display scale, named
// by its third argument and holding the text of the file its fourth names,
// its caret at the start. It writes the file its second argument names,
// empty, from the form's Shown event, once Windows Forms has finished
// showing the window, and exits when the window is closed. mockapp's build
// script compiles it with the .NET Framework's own compiler, so the window
// appears without a script host's start.

using System;
using System.Drawing;
using System.IO;
using System.Windows.Forms;

static class TextBoxForm
{
    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length != 4)
        {
            Console.Error.WriteLine("usage: text_box_form <title> <ready file> <box name> <text file>");
            return 2;
        }
        string ready = args[1];
        Application.EnableVisualStyles();
        var form = new Form();
        form.Text = args[0];
        form.ClientSize = new Size(1100, 400);
        form.StartPosition = FormStartPosition.CenterScreen;
        var box = new TextBox();
        box.Multiline = true;
        box.ScrollBars = ScrollBars.Both;
        box.WordWrap = false;
        box.Dock = DockStyle.Fill;
        box.Font = new Font("Segoe UI", 16);
        box.AccessibleName = args[2];
        box.Text = File.ReadAllText(args[3]);
        box.Select(0, 0);
        form.Controls.Add(box);
        // The box shows thirteen and a half lines of its font at any
        // display scale, as its 400 pixels showed at 100%, so Page Down
        // moves by the same eleven lines the page-keys scenarios state: the
        // font scales with the display and a size in pixels does not, so a
        // larger scale showed fewer lines and moved by fewer.
        int chrome = form.ClientSize.Height - box.ClientSize.Height;
        form.ClientSize = new Size(1100, chrome + box.Font.Height * 27 / 2);
        form.Shown += (sender, e) => File.WriteAllText(ready, "");
        form.ShowDialog();
        return 0;
    }
}
