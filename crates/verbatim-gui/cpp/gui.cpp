// The C++ half of Verbatim's GUI: wxWidgets widgets and their wiring, and
// nothing else. Rust owns main, the event loop's lifetime, and every
// decision: which dialogs open, what a key does, what a setting change sets.
// This file builds widgets from the page models Rust hands over (the shared
// structs in crates/verbatim-gui/src/bridge.rs) and calls the opaque
// GuiCore when the user acts.
//
// Threading: everything here runs on the GUI thread, except
// wake_event_loop, which any thread may call. It queues a call to
// GuiCore::drain with CallAfter, guarded by a mutex and the application
// pointer that OnExit clears, so a wake after the loop has ended does
// nothing.
//
// Reentrancy: popping the menu and showing the Select Synthesizer dialog
// run nested event loops, inside which Rust may be called again (a menu
// choice, a drained command). Rust takes care never to hold a borrow across
// a call into this file; here, state is re-read after every nested loop.
//
// Encoding: narrow string literals stay ASCII. Text from Rust is UTF-8 and
// always goes through Text(), never through wxString's narrow constructor,
// which would read it in the ANSI code page.

#include <wx/wx.h>
#include <wx/filedlg.h>
#include <wx/listctrl.h>
#include <wx/taskbar.h>
#include <wx/textdlg.h>
#include <wx/treectrl.h>

#include <cstddef>
#include <cstdint>
#include <exception>
#include <map>
#include <mutex>
#include <string>
#include <vector>

#include "verbatim-gui/cpp/gui.h"
#include "verbatim-gui/src/bridge.rs.h"

namespace verbatim_gui {
namespace {

wxString Text(const rust::String& text) {
    return wxString::FromUTF8(text.data(), text.size());
}

// Text for Rust: UTF-8, kept alive by the caller while Rust reads it.
std::string Utf8(const wxString& text) {
    const wxScopedCharBuffer utf8 = text.utf8_str();
    return std::string(utf8.data(), utf8.length());
}

rust::Str Str(const std::string& utf8) {
    return rust::Str(utf8.data(), utf8.size());
}

// Menu item ids, above wxWidgets' own range so they never collide with it.
constexpr int kMenuSettings = wxID_HIGHEST + 1001;
constexpr int kMenuExit = wxID_HIGHEST + 1002;

class TrayIcon;
class SettingsDialogWindow;
class ListDialogWindow;

// The GUI's widgets, alive for one run of the event loop.
struct Shell {
    explicit Shell(const GuiCore& core_, const ShellText& text)
        : core(core_),
          title(Text(text.title)),
          tooltip(Text(text.tooltip)),
          settings_item(Text(text.settings_item)),
          exit_item(Text(text.exit_item)) {}

    const GuiCore& core;
    wxString title;
    wxString tooltip;
    wxString settings_item;
    wxString exit_item;
    // The hidden main frame: the dialogs' parent and the single-instance
    // rendezvous window, found by its title.
    wxFrame* frame = nullptr;
    TrayIcon* tray = nullptr;
    // The menu Verbatim+V pops from the frame.
    wxMenu* menu = nullptr;
    SettingsDialogWindow* settings = nullptr;
    ListDialogWindow* list = nullptr;
    // The Select Synthesizer dialog while it runs modally.
    wxDialog* modal = nullptr;
    bool shutting_down = false;
};

// GUI thread only. Set for the duration of run_event_loop.
Shell* g_shell = nullptr;

// Guards g_wake_app, which wake_event_loop reads from any thread.
std::mutex g_wake_mutex;
wxApp* g_wake_app = nullptr;

// The Verbatim menu: Settings, a separator, Exit.
wxMenu* BuildMenu() {
    auto* menu = new wxMenu;
    menu->Append(kMenuSettings, g_shell->settings_item);
    menu->AppendSeparator();
    menu->Append(kMenuExit, g_shell->exit_item);
    return menu;
}

void OnMenu(wxCommandEvent& event) {
    switch (event.GetId()) {
    case kMenuSettings:
        g_shell->core.menu_chosen(MenuChoice::Settings);
        break;
    case kMenuExit:
        g_shell->core.menu_chosen(MenuChoice::Exit);
        break;
    default:
        event.Skip();
    }
}

// The tray icon. A right click pops a fresh copy of the menu, which
// wxWidgets deletes afterwards; its choices arrive as menu events on the
// icon.
class TrayIcon : public wxTaskBarIcon {
protected:
    wxMenu* CreatePopupMenu() override { return BuildMenu(); }
};

void ClickButton(wxButton* button) {
    wxCommandEvent click(wxEVT_BUTTON, button->GetId());
    click.SetEventObject(button);
    button->Command(click);
}

void CloseDialog(DialogKind kind);

// The Speech page: the synthesizer group (a read-only field naming the
// synthesizer and a Change button) above the controls generated from the
// settings host's descriptors, which live in their own panel so a
// synthesizer change can replace them wholesale.
class SpeechPanel : public wxPanel {
public:
    SpeechPanel(wxWindow* parent, wxWindow* dialog)
        : wxPanel(parent), dialog_(dialog) {
        const SpeechPage page = g_shell->core.speech_page();
        sizer_ = new wxBoxSizer(wxVERTICAL);

        auto* group = new wxStaticBoxSizer(wxVERTICAL, this, Text(page.synthesizer_group));
        auto* row = new wxBoxSizer(wxHORIZONTAL);
        name_ = new wxTextCtrl(this, wxID_ANY, Text(page.synthesizer_name), wxDefaultPosition,
                               wxDefaultSize, wxTE_READONLY);
        // The field has no label control of its own; the group names it.
        // Without a name it is announced as a bare "edit".
        name_->SetName(Text(page.synthesizer_field_name));
        change_ = new wxButton(this, wxID_ANY, Text(page.change));
        row->Add(name_, 1, wxEXPAND | wxALL, 5);
        row->Add(change_, 0, wxALL, 5);
        group->Add(row, 0, wxEXPAND | wxALL, 5);
        sizer_->Add(group, 0, wxEXPAND | wxALL, 5);

        controls_ = BuildControls(page);
        sizer_->Add(controls_, 1, wxEXPAND | wxALL, 5);
        SetSizer(sizer_);

        change_->Bind(wxEVT_BUTTON, [this](wxCommandEvent&) { ChangeSynthesizer(); });
    }

    wxButton* change() const { return change_; }
    wxTextCtrl* name() const { return name_; }

private:
    wxPanel* BuildControls(const SpeechPage& page) {
        auto* panel = new wxPanel(this);
        auto* sizer = new wxBoxSizer(wxVERTICAL);
        const std::uint32_t generation = page.generation;
        for (std::size_t index = 0; index < page.controls.size(); ++index) {
            const SettingControl& control = page.controls[index];
            if (control.kind == ControlKind::Slider) {
                auto* label = new wxStaticText(panel, wxID_ANY, Text(control.label));
                auto* slider =
                    new wxSlider(panel, wxID_ANY, control.value, control.min, control.max);
                slider->SetLineSize(control.line_size);
                slider->SetPageSize(control.page_size);
                sizer->Add(label, 0, wxLEFT | wxTOP, 3);
                sizer->Add(slider, 0, wxEXPAND | wxALL, 3);
                slider->Bind(wxEVT_SLIDER, [generation, index, slider](wxCommandEvent&) {
                    g_shell->core.slider_changed(generation, index, slider->GetValue());
                });
            } else if (control.kind == ControlKind::Choice) {
                auto* label = new wxStaticText(panel, wxID_ANY, Text(control.label));
                auto* choice = new wxChoice(panel, wxID_ANY);
                // Frozen while filled: wxWidgets otherwise resizes the
                // dropdown after every entry, which took up to half a second
                // for eSpeak NG's voices. Thawing does not resize it, so it
                // is resized once, to its own size, afterwards.
                choice->Freeze();
                for (const rust::String& option : control.options) {
                    choice->Append(Text(option));
                }
                choice->Thaw();
                choice->SetSize(choice->GetSize());
                if (control.selection >= 0) {
                    choice->SetSelection(control.selection);
                }
                sizer->Add(label, 0, wxLEFT | wxTOP, 3);
                sizer->Add(choice, 0, wxEXPAND | wxALL, 3);
                choice->Bind(wxEVT_CHOICE, [generation, index, choice](wxCommandEvent&) {
                    const int selection = choice->GetSelection();
                    if (selection != wxNOT_FOUND) {
                        g_shell->core.choice_changed(generation, index,
                                                     static_cast<std::size_t>(selection));
                    }
                });
            } else if (control.kind == ControlKind::Toggle) {
                auto* check = new wxCheckBox(panel, wxID_ANY, Text(control.label));
                check->SetValue(control.checked);
                // A check box has no separate label to be named by; without
                // a name it is announced by wxWidgets' default, "check".
                check->SetName(Text(control.name));
                sizer->Add(check, 0, wxALL, 3);
                check->Bind(wxEVT_CHECKBOX, [generation, index](wxCommandEvent& event) {
                    g_shell->core.toggle_changed(generation, index, event.IsChecked());
                });
            }
        }
        panel->SetSizer(sizer);
        return panel;
    }

    // Runs the modal Select Synthesizer dialog and, when the active
    // synthesizer changed, rebuilds the generated controls for it.
    void ChangeSynthesizer() {
        const SynthesizerPicker picker = g_shell->core.synthesizer_picker();
        bool changed = false;
        {
            wxDialog dialog(dialog_, wxID_ANY, Text(picker.title), wxDefaultPosition,
                            wxDefaultSize, wxDEFAULT_DIALOG_STYLE);
            auto* outer = new wxBoxSizer(wxVERTICAL);
            auto* label = new wxStaticText(&dialog, wxID_ANY, Text(picker.label));
            auto* choice = new wxChoice(&dialog, wxID_ANY);
            for (const rust::String& name : picker.names) {
                choice->Append(Text(name));
            }
            if (picker.active < picker.names.size()) {
                choice->SetSelection(static_cast<int>(picker.active));
            }
            outer->Add(label, 0, wxALL, 8);
            outer->Add(choice, 0, wxEXPAND | wxALL, 8);

            auto* buttons = new wxStdDialogButtonSizer();
            auto* ok = new wxButton(&dialog, wxID_OK, Text(picker.ok));
            auto* cancel = new wxButton(&dialog, wxID_CANCEL, Text(picker.cancel));
            buttons->AddButton(ok);
            buttons->AddButton(cancel);
            buttons->Realize();
            outer->Add(buttons, 0, wxEXPAND | wxALL, 8);

            dialog.SetSizer(outer);
            dialog.SetAffirmativeId(wxID_OK);
            dialog.SetEscapeId(wxID_CANCEL);
            ok->SetDefault();
            dialog.Fit();
            dialog.Centre();

            g_shell->modal = &dialog;
            const int result = dialog.ShowModal();
            g_shell->modal = nullptr;
            // Shutdown can arrive during the modal loop; it ends the loop
            // and destroys the settings dialog, so nothing more is done.
            if (g_shell->shutting_down) {
                return;
            }
            const int selection = choice->GetSelection();
            if (result == wxID_OK && selection != wxNOT_FOUND) {
                changed = g_shell->core.choose_synthesizer(static_cast<std::size_t>(selection));
            }
        }
        if (changed) {
            const SpeechPage page = g_shell->core.speech_page();
            // Hidden first so the old controls give up their share of the
            // sizer at once; destroying a child window detaches it.
            controls_->Hide();
            controls_->Destroy();
            controls_ = BuildControls(page);
            sizer_->Add(controls_, 1, wxEXPAND | wxALL, 5);
            name_->SetValue(Text(page.synthesizer_name));
            Layout();
        }
    }

    wxWindow* dialog_;
    wxBoxSizer* sizer_ = nullptr;
    wxTextCtrl* name_ = nullptr;
    wxButton* change_ = nullptr;
    wxPanel* controls_ = nullptr;
};

// Puts `items` in `choice` and selects `selection` (-1 for none), touching
// the widget only where it differs, so a focused choice keeps its place and
// is not announced again.
void SetChoice(wxChoice* choice, const rust::Vec<rust::String>& items, std::int32_t selection) {
    bool same = choice->GetCount() == items.size();
    for (std::size_t index = 0; same && index < items.size(); ++index) {
        same = choice->GetString(static_cast<unsigned int>(index)) == Text(items[index]);
    }
    if (!same) {
        choice->Freeze();
        choice->Clear();
        for (const rust::String& item : items) {
            choice->Append(Text(item));
        }
        choice->Thaw();
    }
    const int wanted = selection >= 0 ? selection : wxNOT_FOUND;
    if (choice->GetSelection() != wanted) {
        choice->SetSelection(wanted);
    }
}

// Sets a text field's value without raising a change event, when it differs.
void SetValue(wxTextCtrl* text, const rust::String& value) {
    const wxString wanted = Text(value);
    if (text->GetValue() != wanted) {
        text->ChangeValue(wanted);
    }
}

// A tree item's indication, by its index in the catalogue; -1 for a
// category.
class IndicationData : public wxTreeItemData {
public:
    explicit IndicationData(std::int64_t indication) : indication_(indication) {}
    std::int64_t indication() const { return indication_; }

private:
    std::int64_t indication_;
};

// The Theme page (phase6-design.md, "The settings dialog"): the theme list,
// its description, the settings that go with it, the indications tree with
// a find field, the selected indication's controls, and the theme buttons.
// Rust holds the state (crates/verbatim-gui/src/theme_panel.rs); after every
// change this asks it for the page again and updates what differs.
class ThemePanel : public wxPanel {
public:
    ThemePanel(wxWindow* parent, wxWindow* dialog) : wxPanel(parent), dialog_(dialog) {
        page_ = g_shell->core.theme_page();
        auto* sizer = new wxBoxSizer(wxVERTICAL);

        // Each label is made just before its control, so the control is
        // named by it and the tab order follows the page from top to bottom.
        auto* top = new wxBoxSizer(wxHORIZONTAL);
        top->Add(new wxStaticText(this, wxID_ANY, Text(page_.theme_label)), 0,
                 wxALIGN_CENTER_VERTICAL | wxALL, 3);
        theme_ = new wxChoice(this, wxID_ANY);
        top->Add(theme_, 1, wxEXPAND | wxALL, 3);
        sizer->Add(top, 0, wxEXPAND);

        sizer->Add(new wxStaticText(this, wxID_ANY, Text(page_.description_label)), 0,
                   wxLEFT | wxTOP, 3);
        description_ = new wxTextCtrl(this, wxID_ANY, wxString(), wxDefaultPosition,
                                      wxSize(-1, 60), wxTE_MULTILINE | wxTE_READONLY);
        sizer->Add(description_, 0, wxEXPAND | wxALL, 3);

        auto* options = new wxBoxSizer(wxHORIZONTAL);
        options->Add(new wxStaticText(this, wxID_ANY, Text(page_.volume_label)), 0,
                     wxALIGN_CENTER_VERTICAL | wxALL, 3);
        volume_ = new wxSlider(this, wxID_ANY, page_.volume, 0, 100);
        volume_->SetLineSize(5);
        volume_->SetPageSize(10);
        options->Add(volume_, 1, wxEXPAND | wxALL, 3);
        say_all_ = new wxCheckBox(this, wxID_ANY, Text(page_.say_all_label));
        // A check box has no separate label to be named by.
        say_all_->SetName(Text(page_.say_all_name));
        options->Add(say_all_, 0, wxALIGN_CENTER_VERTICAL | wxALL, 3);
        speak_sounded_ = new wxCheckBox(this, wxID_ANY, Text(page_.speak_sounded_label));
        speak_sounded_->SetName(Text(page_.speak_sounded_name));
        options->Add(speak_sounded_, 0, wxALIGN_CENTER_VERTICAL | wxALL, 3);
        sizer->Add(options, 0, wxEXPAND);

        auto* body = new wxBoxSizer(wxHORIZONTAL);
        auto* left = new wxBoxSizer(wxVERTICAL);
        left->Add(new wxStaticText(this, wxID_ANY, Text(page_.find_label)), 0, wxLEFT | wxTOP, 3);
        find_ = new wxTextCtrl(this, wxID_ANY);
        left->Add(find_, 0, wxEXPAND | wxALL, 3);
        left->Add(new wxStaticText(this, wxID_ANY, Text(page_.indications_label)), 0,
                  wxLEFT | wxTOP, 3);
        tree_ = new wxTreeCtrl(this, wxID_ANY, wxDefaultPosition, wxSize(-1, 160),
                               wxTR_HIDE_ROOT | wxTR_HAS_BUTTONS | wxTR_LINES_AT_ROOT |
                                   wxTR_SINGLE);
        left->Add(tree_, 1, wxEXPAND | wxALL, 3);
        body->Add(left, 1, wxEXPAND);

        auto* right = new wxBoxSizer(wxVERTICAL);
        right->Add(new wxStaticText(this, wxID_ANY, Text(page_.report_label)), 0,
                   wxLEFT | wxTOP, 3);
        report_ = new wxChoice(this, wxID_ANY);
        right->Add(report_, 0, wxEXPAND | wxALL, 3);
        right->Add(new wxStaticText(this, wxID_ANY, Text(page_.sound_label)), 0, wxLEFT | wxTOP,
                   3);
        sound_ = new wxChoice(this, wxID_ANY);
        right->Add(sound_, 0, wxEXPAND | wxALL, 3);
        right->Add(new wxStaticText(this, wxID_ANY, Text(page_.words_label)), 0, wxLEFT | wxTOP,
                   3);
        words_ = new wxTextCtrl(this, wxID_ANY);
        right->Add(words_, 0, wxEXPAND | wxALL, 3);
        right->Add(new wxStaticText(this, wxID_ANY, Text(page_.voice_label)), 0, wxLEFT | wxTOP,
                   3);
        voice_ = new wxChoice(this, wxID_ANY);
        right->Add(voice_, 0, wxEXPAND | wxALL, 3);
        auto* indication_buttons = new wxBoxSizer(wxHORIZONTAL);
        preview_ = new wxButton(this, wxID_ANY, Text(page_.preview));
        reset_ = new wxButton(this, wxID_ANY, Text(page_.reset));
        indication_buttons->Add(preview_, 0, wxALL, 3);
        indication_buttons->Add(reset_, 0, wxALL, 3);
        right->Add(indication_buttons, 0);
        body->Add(right, 1, wxEXPAND);
        sizer->Add(body, 1, wxEXPAND);

        auto* theme_buttons = new wxBoxSizer(wxHORIZONTAL);
        new_ = new wxButton(this, wxID_ANY, Text(page_.new_theme));
        rename_ = new wxButton(this, wxID_ANY, Text(page_.rename));
        import_ = new wxButton(this, wxID_ANY, Text(page_.import_label));
        export_ = new wxButton(this, wxID_ANY, Text(page_.export_label));
        remove_ = new wxButton(this, wxID_ANY, Text(page_.remove));
        for (wxButton* button : {new_, rename_, import_, export_, remove_}) {
            theme_buttons->Add(button, 0, wxALL, 3);
        }
        sizer->Add(theme_buttons, 0);
        SetSizer(sizer);

        Wire();
        RebuildTree();
        RefreshAll();
    }

    wxChoice* sound() const { return sound_; }

    // Space on the sound choice: plays the sound it shows.
    void PlaySound() { g_shell->core.play_indication_sound(); }

private:
    // Connects every control to what Rust does with it.
    void Wire() {
        theme_->Bind(wxEVT_CHOICE, [this](wxCommandEvent&) {
            const int selection = theme_->GetSelection();
            if (selection != wxNOT_FOUND) {
                g_shell->core.theme_chosen(static_cast<std::size_t>(selection));
                RefreshAll();
            }
        });
        volume_->Bind(wxEVT_SLIDER,
                      [this](wxCommandEvent&) { g_shell->core.volume_changed(volume_->GetValue()); });
        say_all_->Bind(wxEVT_CHECKBOX, [](wxCommandEvent& event) {
            g_shell->core.say_all_changed(event.IsChecked());
        });
        speak_sounded_->Bind(wxEVT_CHECKBOX, [](wxCommandEvent& event) {
            g_shell->core.speak_sounded_changed(event.IsChecked());
        });
        find_->Bind(wxEVT_TEXT, [this](wxCommandEvent&) {
            const std::string text = Utf8(find_->GetValue());
            g_shell->core.theme_filter_changed(Str(text));
            RebuildTree();
        });
        tree_->Bind(wxEVT_TREE_SEL_CHANGED, [this](wxTreeEvent& event) {
            if (!rebuilding_) {
                SelectIndication(event.GetItem());
            }
        });
        report_->Bind(wxEVT_CHOICE, [this](wxCommandEvent&) {
            const int selection = report_->GetSelection();
            if (selection != wxNOT_FOUND) {
                Edit(g_shell->core.report_changed(static_cast<std::size_t>(selection)));
            }
        });
        sound_->Bind(wxEVT_CHOICE, [this](wxCommandEvent&) {
            const int selection = sound_->GetSelection();
            if (selection == wxNOT_FOUND) {
                return;
            }
            if (selection == sound_browse_) {
                BrowseSound();
            } else {
                Edit(g_shell->core.sound_changed(static_cast<std::size_t>(selection)));
            }
        });
        words_->Bind(wxEVT_TEXT, [this](wxCommandEvent&) {
            const std::string text = Utf8(words_->GetValue());
            Edit(g_shell->core.words_changed(Str(text)));
        });
        voice_->Bind(wxEVT_CHOICE, [this](wxCommandEvent&) {
            const int selection = voice_->GetSelection();
            if (selection != wxNOT_FOUND) {
                Edit(g_shell->core.voice_changed(static_cast<std::size_t>(selection)));
            }
        });
        preview_->Bind(wxEVT_BUTTON, [](wxCommandEvent&) { g_shell->core.preview_indication(); });
        reset_->Bind(wxEVT_BUTTON,
                     [this](wxCommandEvent&) { Edit(g_shell->core.reset_indication()); });
        new_->Bind(wxEVT_BUTTON, [this](wxCommandEvent&) {
            std::string name;
            if (AskName(page_.prompts.new_prompt, page_.prompts.new_title, page_.prompts.new_name,
                        name)) {
                Report(g_shell->core.new_theme(Str(name)));
            }
        });
        rename_->Bind(wxEVT_BUTTON, [this](wxCommandEvent&) {
            std::string name;
            if (AskName(page_.prompts.rename_prompt, page_.prompts.rename_title,
                        page_.prompts.name, name)) {
                Report(g_shell->core.rename_theme(Str(name)));
            }
        });
        import_->Bind(wxEVT_BUTTON, [this](wxCommandEvent&) {
            std::string path;
            if (AskFile(page_.prompts.import_title, rust::String(), page_.prompts.package_filter,
                        wxFD_OPEN | wxFD_FILE_MUST_EXIST, path)) {
                Report(g_shell->core.import_theme(Str(path)));
            }
        });
        export_->Bind(wxEVT_BUTTON, [this](wxCommandEvent&) {
            std::string path;
            if (AskFile(page_.prompts.export_title, page_.prompts.export_file,
                        page_.prompts.package_filter, wxFD_SAVE | wxFD_OVERWRITE_PROMPT, path)) {
                Report(g_shell->core.export_theme(Str(path)));
            }
        });
        remove_->Bind(wxEVT_BUTTON, [this](wxCommandEvent&) {
            // A native message box, which shutdown cannot end early: it
            // only checks for shutdown once the box has closed.
            wxMessageDialog confirm(dialog_, Text(page_.prompts.remove_question),
                                    Text(page_.prompts.remove_title),
                                    wxYES_NO | wxNO_DEFAULT | wxICON_QUESTION);
            const int answer = confirm.ShowModal();
            if (g_shell->shutting_down) {
                return;
            }
            if (answer == wxID_YES) {
                Report(g_shell->core.remove_theme());
            }
        });
    }

    // Asks for a name in a modal prompt starting with `initial`. False when
    // it was cancelled, or Verbatim is shutting down.
    bool AskName(const rust::String& prompt, const rust::String& title,
                 const rust::String& initial, std::string& name) {
        wxTextEntryDialog ask(dialog_, Text(prompt), Text(title), Text(initial));
        g_shell->modal = &ask;
        const int answer = ask.ShowModal();
        g_shell->modal = nullptr;
        if (g_shell->shutting_down || answer != wxID_OK) {
            return false;
        }
        name = Utf8(ask.GetValue());
        return true;
    }

    // Asks for a file in the system's file dialog. False when it was
    // cancelled, or Verbatim is shutting down.
    bool AskFile(const rust::String& title, const rust::String& file, const rust::String& filter,
                 long style, std::string& path) {
        wxFileDialog ask(dialog_, Text(title), wxString(), Text(file), Text(filter), style);
        const int answer = ask.ShowModal();
        if (g_shell->shutting_down || answer != wxID_OK) {
            return false;
        }
        path = Utf8(ask.GetPath());
        return true;
    }

    // Shows why an operation failed, if it did, then the page as it now is.
    void Report(const rust::String& error) {
        if (!error.empty()) {
            wxMessageDialog message(dialog_, Text(error), Text(page_.prompts.error_title),
                                    wxOK | wxICON_ERROR);
            message.ShowModal();
            if (g_shell->shutting_down) {
                return;
            }
        }
        RefreshAll();
    }

    // Carries out what became of a change to an indication: asks for a new
    // theme's name when the change is to the built-in theme, then reports.
    void Edit(ThemeEdit edit) {
        if (edit.needs_name) {
            std::string name;
            const bool accepted = AskName(edit.prompt, edit.prompt_title, edit.suggested_name, name);
            if (g_shell->shutting_down) {
                return;
            }
            edit = g_shell->core.theme_named(Str(name), accepted);
        }
        Report(edit.error);
    }

    void BrowseSound() {
        std::string path;
        if (AskFile(page_.prompts.sound_title, rust::String(), page_.prompts.sound_filter,
                    wxFD_OPEN | wxFD_FILE_MUST_EXIST, path)) {
            Edit(g_shell->core.sound_browsed(Str(path)));
        } else if (!g_shell->shutting_down) {
            RefreshControls();
        }
    }

    void SelectIndication(const wxTreeItemId& item) {
        std::int64_t indication = -1;
        if (item.IsOk()) {
            if (const auto* data = dynamic_cast<IndicationData*>(tree_->GetItemData(item))) {
                indication = data->indication();
            }
        }
        g_shell->core.indication_selected(indication);
        RefreshControls();
    }

    // Builds the tree afresh, as the find field filters it, keeping the
    // selected indication selected when it is still listed.
    void RebuildTree() {
        std::int64_t selected = -1;
        const wxTreeItemId current = tree_->GetSelection();
        if (current.IsOk()) {
            if (const auto* data = dynamic_cast<IndicationData*>(tree_->GetItemData(current))) {
                selected = data->indication();
            }
        }
        const rust::Vec<ThemeTreeCategory> categories = g_shell->core.theme_tree();
        const bool filtered = !find_->GetValue().empty();
        rebuilding_ = true;
        tree_->Freeze();
        tree_->DeleteAllItems();
        items_.clear();
        const wxTreeItemId root = tree_->AddRoot(wxString());
        wxTreeItemId reselect;
        for (const ThemeTreeCategory& category : categories) {
            const wxTreeItemId parent =
                tree_->AppendItem(root, Text(category.label), -1, -1, new IndicationData(-1));
            for (const ThemeTreeItem& item : category.items) {
                const auto indication = static_cast<std::int64_t>(item.indication);
                const wxTreeItemId id = tree_->AppendItem(parent, Text(item.label), -1, -1,
                                                          new IndicationData(indication));
                items_[indication] = id;
                if (indication == selected) {
                    reselect = id;
                }
            }
            if (filtered) {
                tree_->Expand(parent);
            }
        }
        if (reselect.IsOk()) {
            tree_->SelectItem(reselect);
            tree_->EnsureVisible(reselect);
        }
        tree_->Thaw();
        rebuilding_ = false;
        SelectIndication(tree_->GetSelection());
    }

    // Updates the tree's labels after a change, rebuilding it only when the
    // find field now lists other indications.
    void RefreshTree() {
        const rust::Vec<ThemeTreeCategory> categories = g_shell->core.theme_tree();
        std::size_t count = 0;
        bool same = true;
        for (const ThemeTreeCategory& category : categories) {
            for (const ThemeTreeItem& item : category.items) {
                ++count;
                same = same && items_.count(static_cast<std::int64_t>(item.indication)) == 1;
            }
        }
        if (!same || count != items_.size()) {
            RebuildTree();
            return;
        }
        for (const ThemeTreeCategory& category : categories) {
            for (const ThemeTreeItem& item : category.items) {
                const wxTreeItemId id = items_[static_cast<std::int64_t>(item.indication)];
                const wxString label = Text(item.label);
                if (tree_->GetItemText(id) != label) {
                    tree_->SetItemText(id, label);
                }
            }
        }
    }

    // The page, the tree's labels, and the indication's controls as Rust
    // now has them.
    void RefreshAll() {
        page_ = g_shell->core.theme_page();
        SetChoice(theme_, page_.themes, page_.selected);
        SetValue(description_, page_.description);
        if (volume_->GetValue() != page_.volume) {
            volume_->SetValue(page_.volume);
        }
        if (say_all_->GetValue() != page_.say_all) {
            say_all_->SetValue(page_.say_all);
        }
        if (speak_sounded_->GetValue() != page_.speak_sounded) {
            speak_sounded_->SetValue(page_.speak_sounded);
        }
        rename_->Enable(page_.can_rename);
        export_->Enable(page_.can_rename);
        remove_->Enable(page_.can_remove);
        RefreshTree();
        RefreshControls();
    }

    void RefreshControls() {
        const IndicationControls controls = g_shell->core.indication_controls();
        SetChoice(report_, controls.report_options, controls.report);
        report_->Enable(controls.report_enabled);
        SetChoice(sound_, controls.sound_options, controls.sound);
        sound_->Enable(controls.sound_enabled);
        sound_browse_ = controls.sound_browse;
        SetValue(words_, controls.words);
        words_->Enable(controls.words_enabled);
        SetChoice(voice_, controls.voice_options, controls.voice);
        voice_->Enable(controls.voice_enabled);
        preview_->Enable(controls.preview_enabled);
        reset_->Enable(controls.reset_enabled);
    }

    wxWindow* dialog_;
    ThemePage page_;
    wxChoice* theme_ = nullptr;
    wxTextCtrl* description_ = nullptr;
    wxSlider* volume_ = nullptr;
    wxCheckBox* say_all_ = nullptr;
    wxCheckBox* speak_sounded_ = nullptr;
    wxTextCtrl* find_ = nullptr;
    wxTreeCtrl* tree_ = nullptr;
    wxChoice* report_ = nullptr;
    wxChoice* sound_ = nullptr;
    wxTextCtrl* words_ = nullptr;
    wxChoice* voice_ = nullptr;
    wxButton* preview_ = nullptr;
    wxButton* reset_ = nullptr;
    wxButton* new_ = nullptr;
    wxButton* rename_ = nullptr;
    wxButton* import_ = nullptr;
    wxButton* export_ = nullptr;
    wxButton* remove_ = nullptr;
    std::int32_t sound_browse_ = -1;
    // The tree item of each listed indication, by its catalogue index.
    std::map<std::int64_t, wxTreeItemId> items_;
    // Set while the tree is rebuilt, whose selection events are its own.
    bool rebuilding_ = false;
};

// The settings dialog, after NVDA's MultiCategorySettingsDialog: a category
// list on the left swaps the category's page on the right, which is built
// the first time it is shown; OK, Cancel, and Apply sit along the bottom.
// Settings apply live as they change: OK and Apply keep them, Cancel undoes
// them.
class SettingsDialogWindow : public wxDialog {
public:
    SettingsDialogWindow(wxWindow* parent, const SettingsDialog& model)
        : wxDialog(parent, wxID_ANY,
                   model.categories.empty() ? wxString() : Text(model.categories[0].title),
                   wxDefaultPosition, wxSize(880, 620),
                   wxDEFAULT_DIALOG_STYLE | wxRESIZE_BORDER) {
        SetMinSize(wxSize(520, 360));
        for (const Category& category : model.categories) {
            pages_.push_back(Page{Text(category.name), Text(category.title), category.kind});
        }

        auto* outer = new wxBoxSizer(wxVERTICAL);
        auto* content = new wxBoxSizer(wxHORIZONTAL);

        // The label comes before the list, so the list is named by it.
        auto* left = new wxBoxSizer(wxVERTICAL);
        auto* label = new wxStaticText(this, wxID_ANY, Text(model.categories_label));
        list_ = new wxListCtrl(this, wxID_ANY, wxDefaultPosition, wxDefaultSize,
                               wxLC_REPORT | wxLC_SINGLE_SEL | wxLC_NO_HEADER);
        list_->InsertColumn(0, wxString(), wxLIST_FORMAT_LEFT, 200);
        for (std::size_t index = 0; index < pages_.size(); ++index) {
            list_->InsertItem(static_cast<long>(index), pages_[index].name);
        }
        left->Add(label, 0, wxALL, 5);
        left->Add(list_, 1, wxEXPAND | wxALL, 5);
        content->Add(left, 0, wxEXPAND | wxALL, 0);

        container_ = new wxPanel(this);
        container_sizer_ = new wxBoxSizer(wxVERTICAL);
        container_->SetSizer(container_sizer_);
        content->Add(container_, 1, wxEXPAND | wxALL, 5);
        outer->Add(content, 1, wxEXPAND | wxALL, 0);

        auto* buttons = new wxStdDialogButtonSizer();
        ok_ = new wxButton(this, wxID_OK, Text(model.ok));
        cancel_ = new wxButton(this, wxID_CANCEL, Text(model.cancel));
        apply_ = new wxButton(this, wxID_APPLY, Text(model.apply));
        buttons->AddButton(ok_);
        buttons->AddButton(cancel_);
        buttons->AddButton(apply_);
        buttons->Realize();
        outer->Add(buttons, 0, wxEXPAND | wxALL, 5);

        // Bound here, these run before wxDialog's own button handling,
        // which would only hide a modeless dialog. Escape and the close
        // box arrive as a click on Cancel.
        Bind(wxEVT_BUTTON, [](wxCommandEvent&) {
            g_shell->core.commit_settings();
            CloseDialog(DialogKind::Settings);
        }, wxID_OK);
        Bind(wxEVT_BUTTON, [](wxCommandEvent&) {
            g_shell->core.revert_settings();
            CloseDialog(DialogKind::Settings);
        }, wxID_CANCEL);
        Bind(wxEVT_BUTTON, [](wxCommandEvent&) { g_shell->core.commit_settings(); }, wxID_APPLY);
        ok_->SetDefault();
        SetAffirmativeId(wxID_OK);
        SetEscapeId(wxID_CANCEL);

        SetSizer(outer);
        Centre();

        list_->Bind(wxEVT_LIST_ITEM_SELECTED, [this](wxListEvent& event) {
            if (event.GetIndex() >= 0) {
                ActivateCategory(static_cast<std::size_t>(event.GetIndex()), false);
            }
        });
        // One hook for the whole dialog: it sees keys from every child,
        // including the controls inside the category pages.
        Bind(wxEVT_CHAR_HOOK, &SettingsDialogWindow::OnCharHook, this);

        ActivateCategory(0, true);
        list_->SetFocus();
    }

private:
    struct Page {
        wxString name;
        wxString title;
        CategoryKind kind;
        wxPanel* panel = nullptr;
    };

    // Shows the category at `index`, building its page the first time,
    // and keeps the list selection and the title in step. `update_list` is
    // false when the list itself drove the change.
    void ActivateCategory(std::size_t index, bool update_list) {
        if (index >= pages_.size()) {
            return;
        }
        Page& page = pages_[index];
        if (page.panel == nullptr) {
            page.panel = BuildPage(page.kind);
            container_sizer_->Add(page.panel, 1, wxEXPAND | wxALL, 0);
        }
        for (std::size_t other = 0; other < pages_.size(); ++other) {
            if (pages_[other].panel != nullptr) {
                pages_[other].panel->Show(other == index);
            }
        }
        selected_ = index;
        if (update_list) {
            list_->SetItemState(static_cast<long>(index),
                                wxLIST_STATE_SELECTED | wxLIST_STATE_FOCUSED,
                                wxLIST_STATE_SELECTED | wxLIST_STATE_FOCUSED);
        }
        SetTitle(page.title);
        container_->Layout();
        Layout();
    }

    wxPanel* BuildPage(CategoryKind kind) {
        if (kind == CategoryKind::Speech) {
            speech_ = new SpeechPanel(container_, this);
            return speech_;
        }
        if (kind == CategoryKind::Theme) {
            theme_ = new ThemePanel(container_, this);
            return theme_;
        }
        return new wxPanel(container_);
    }

    SettingsFocus FocusOf(const wxWindow* focus) const {
        if (focus == ok_) {
            return SettingsFocus::Ok;
        }
        if (focus == cancel_) {
            return SettingsFocus::Cancel;
        }
        if (focus == apply_) {
            return SettingsFocus::Apply;
        }
        if (speech_ != nullptr && focus == speech_->change()) {
            return SettingsFocus::ChangeSynthesizer;
        }
        if (speech_ != nullptr && focus == speech_->name()) {
            return SettingsFocus::SynthesizerName;
        }
        if (theme_ != nullptr && focus == theme_->sound()) {
            return SettingsFocus::SoundChoice;
        }
        if (dynamic_cast<const wxButton*>(focus) != nullptr) {
            return SettingsFocus::OtherButton;
        }
        return SettingsFocus::Other;
    }

    // Asks Rust what the key does (crates/verbatim-gui/src/keys.rs) and
    // does it; a key Rust passes through goes on to the focused control and
    // wxDialog's own handling, which turns Escape into Cancel.
    void OnCharHook(wxKeyEvent& event) {
        wxWindow* focus = wxWindow::FindFocus();
        if (focus == nullptr || wxGetTopLevelParent(focus) != this) {
            event.Skip();
            return;
        }
        SettingsKey key = SettingsKey::Other;
        switch (event.GetKeyCode()) {
        case WXK_RETURN:
        case WXK_NUMPAD_ENTER:
            key = SettingsKey::Enter;
            break;
        case WXK_TAB:
            key = SettingsKey::Tab;
            break;
        case 'S':
            key = SettingsKey::S;
            break;
        case WXK_SPACE:
            key = SettingsKey::Space;
            break;
        default:
            break;
        }
        const SettingsKeyAction action =
            route_settings_key(key, event.ControlDown(), event.ShiftDown(), FocusOf(focus));
        if (action == SettingsKeyAction::NextCategory ||
            action == SettingsKeyAction::PreviousCategory) {
            ActivateCategory(next_category(selected_, pages_.size(),
                                           action == SettingsKeyAction::NextCategory),
                             true);
            list_->SetFocus();
        } else if (action == SettingsKeyAction::Ok) {
            ClickButton(ok_);
        } else if (action == SettingsKeyAction::Cancel) {
            ClickButton(cancel_);
        } else if (action == SettingsKeyAction::Apply) {
            ClickButton(apply_);
        } else if (action == SettingsKeyAction::ChangeSynthesizer && speech_ != nullptr) {
            ClickButton(speech_->change());
        } else if (action == SettingsKeyAction::ActivateFocused) {
            if (auto* button = dynamic_cast<wxButton*>(focus)) {
                ClickButton(button);
            }
        } else if (action == SettingsKeyAction::PlaySound && theme_ != nullptr) {
            theme_->PlaySound();
        } else {
            event.Skip();
        }
    }

    std::vector<Page> pages_;
    std::size_t selected_ = 0;
    wxListCtrl* list_ = nullptr;
    wxPanel* container_ = nullptr;
    wxBoxSizer* container_sizer_ = nullptr;
    wxButton* ok_ = nullptr;
    wxButton* cancel_ = nullptr;
    wxButton* apply_ = nullptr;
    SpeechPanel* speech_ = nullptr;
    ThemePanel* theme_ = nullptr;
};

// A list dialog: a label over a single-selection list, a row of buttons,
// and Cancel. A button runs against the selected item; Enter or a double
// click on an item runs the default button; Escape and the close box
// arrive as a click on Cancel.
class ListDialogWindow : public wxDialog {
public:
    ListDialogWindow(wxWindow* parent, const ListDialog& model)
        : wxDialog(parent, wxID_ANY, Text(model.title), wxDefaultPosition, wxDefaultSize,
                   wxDEFAULT_DIALOG_STYLE),
          default_button_(model.default_button) {
        auto* outer = new wxBoxSizer(wxVERTICAL);
        auto* label = new wxStaticText(this, wxID_ANY, Text(model.label));
        list_ = new wxListBox(this, wxID_ANY, wxDefaultPosition, wxSize(550, 250));
        // The label does not name the list on its own; without a name the
        // list is announced as a bare "list".
        list_->SetName(Text(model.list_name));
        for (const rust::String& item : model.items) {
            list_->Append(Text(item));
        }
        if (model.selection >= 0) {
            list_->SetSelection(model.selection);
        }
        outer->Add(label, 0, wxLEFT | wxALL, 8);
        outer->Add(list_, 1, wxEXPAND | wxALL, 8);

        auto* row = new wxBoxSizer(wxHORIZONTAL);
        for (std::size_t index = 0; index < model.buttons.size(); ++index) {
            auto* button = new wxButton(this, wxID_ANY, Text(model.buttons[index]));
            row->Add(button, 0, wxALL, 5);
            button->Bind(wxEVT_BUTTON, [this, index](wxCommandEvent&) { Activate(index); });
            buttons_.push_back(button);
        }
        auto* cancel = new wxButton(this, wxID_CANCEL, Text(model.cancel));
        row->Add(cancel, 0, wxALL, 5);
        outer->Add(row, 0, wxALIGN_RIGHT | wxALL, 5);

        Bind(wxEVT_BUTTON, [](wxCommandEvent&) { CloseDialog(DialogKind::ShellList); },
             wxID_CANCEL);
        list_->Bind(wxEVT_LISTBOX_DCLICK, [this](wxCommandEvent&) { Activate(default_button_); });
        list_->Bind(wxEVT_KEY_DOWN, [this](wxKeyEvent& event) {
            if (event.GetKeyCode() == WXK_RETURN) {
                Activate(default_button_);
                return;
            }
            event.Skip();
        });
        if (default_button_ < buttons_.size()) {
            buttons_[default_button_]->SetDefault();
        }
        SetEscapeId(wxID_CANCEL);

        SetSizer(outer);
        Fit();
        Centre();
        list_->SetFocus();
    }

private:
    // Runs button `index` against the selected item; with no selection it
    // does nothing.
    void Activate(std::size_t index) {
        if (index >= buttons_.size()) {
            return;
        }
        const int selection = list_->GetSelection();
        if (selection == wxNOT_FOUND) {
            return;
        }
        if (g_shell->core.list_button(index, static_cast<std::size_t>(selection))) {
            CloseDialog(DialogKind::ShellList);
        }
    }

    wxListBox* list_ = nullptr;
    std::vector<wxButton*> buttons_;
    std::size_t default_button_;
};

wxDialog* DialogOf(DialogKind kind) {
    if (g_shell == nullptr) {
        return nullptr;
    }
    if (kind == DialogKind::Settings) {
        return g_shell->settings;
    }
    if (kind == DialogKind::ShellList) {
        return g_shell->list;
    }
    return nullptr;
}

// The one way a dialog closes: destroy it, forget it, and tell Rust.
void CloseDialog(DialogKind kind) {
    wxDialog* dialog = DialogOf(kind);
    if (dialog == nullptr) {
        return;
    }
    if (kind == DialogKind::Settings) {
        g_shell->settings = nullptr;
    } else {
        g_shell->list = nullptr;
    }
    dialog->Destroy();
    g_shell->core.dialog_closed(kind);
}

class App : public wxApp {
public:
    explicit App(Shell& shell) : shell_(shell) {}

    // The base OnInit only parses the command line, which belongs to Rust.
    bool OnInit() override {
        // Rust decides when the loop ends (shut_down); deleting a dialog or
        // the frame must not end it.
        SetExitOnFrameDelete(false);

        shell_.frame = new wxFrame(nullptr, wxID_ANY, shell_.title, wxDefaultPosition,
                                   wxSize(1, 1), wxDEFAULT_FRAME_STYLE | wxFRAME_NO_TASKBAR);
        shell_.frame->Bind(wxEVT_CLOSE_WINDOW, [](wxCloseEvent& event) {
            // Closing the hidden frame hides it; only shutdown lets the
            // default handling destroy it.
            if (g_shell->shutting_down) {
                event.Skip();
            } else if (g_shell->frame != nullptr) {
                g_shell->frame->Hide();
            }
        });
        shell_.frame->Bind(wxEVT_MENU, &OnMenu);
        shell_.menu = BuildMenu();

        shell_.tray = new TrayIcon;
        // A blank, fully transparent icon: there is no artwork yet.
        wxImage image(16, 16, true);
        image.InitAlpha();
        unsigned char* alpha = image.GetAlpha();
        for (int pixel = 0; pixel < 16 * 16; ++pixel) {
            alpha[pixel] = 0;
        }
        shell_.tray->SetIcon(wxBitmapBundle(wxBitmap(image)), shell_.tooltip);
        shell_.tray->Bind(wxEVT_MENU, &OnMenu);
        shell_.tray->Bind(wxEVT_TASKBAR_LEFT_DOWN,
                          [](wxTaskBarIconEvent&) { g_shell->core.tray_clicked(); });

        {
            std::lock_guard<std::mutex> lock(g_wake_mutex);
            g_wake_app = this;
        }
        shell_.core.ready();
        return true;
    }

    int OnExit() override {
        {
            std::lock_guard<std::mutex> lock(g_wake_mutex);
            g_wake_app = nullptr;
        }
        if (shell_.tray != nullptr) {
            shell_.tray->RemoveIcon();
            delete shell_.tray;
            shell_.tray = nullptr;
        }
        delete shell_.menu;
        shell_.menu = nullptr;
        return wxApp::OnExit();
    }

private:
    Shell& shell_;
};

} // namespace

std::int32_t run_event_loop(const GuiCore& core, const ShellText& text) {
    Shell shell(core, text);
    g_shell = &shell;
    // wxEntryStart takes ownership of the application object.
    wxApp::SetInstance(new App(shell));
    char name[] = "verbatim";
    char* argv[] = {name, nullptr};
    int argc = 1;
    if (!wxEntryStart(argc, argv)) {
        g_shell = nullptr;
        return -1;
    }
    std::int32_t code = -1;
    bool initialized = false;
    try {
        initialized = wxTheApp->CallOnInit();
        if (initialized) {
            code = wxTheApp->OnRun();
        }
    } catch (const std::exception& error) {
        wxLogError("Unhandled exception in the GUI: %s", error.what());
    } catch (...) {
        wxLogError("Unhandled exception in the GUI");
    }
    if (initialized) {
        wxTheApp->OnExit();
    }
    wxEntryCleanup();
    g_shell = nullptr;
    return code;
}

void wake_event_loop() {
    std::lock_guard<std::mutex> lock(g_wake_mutex);
    if (g_wake_app != nullptr) {
        g_wake_app->CallAfter([] {
            if (g_shell != nullptr) {
                g_shell->core.drain();
            }
        });
    }
}

std::size_t frame_handle() {
    if (g_shell == nullptr || g_shell->frame == nullptr) {
        return 0;
    }
    return reinterpret_cast<std::size_t>(g_shell->frame->GetHWND());
}

ScreenPoint centre_frame() {
    if (g_shell == nullptr || g_shell->frame == nullptr) {
        return ScreenPoint{0, 0};
    }
    g_shell->frame->CentreOnScreen();
    const wxPoint position = g_shell->frame->GetPosition();
    return ScreenPoint{position.x, position.y};
}

void show_frame() {
    if (g_shell != nullptr && g_shell->frame != nullptr) {
        g_shell->frame->Show(true);
        g_shell->frame->Raise();
    }
}

void hide_frame() {
    if (g_shell != nullptr && g_shell->frame != nullptr) {
        g_shell->frame->Hide();
    }
}

bool popup_menu(ScreenPoint at) {
    if (g_shell == nullptr || g_shell->frame == nullptr || g_shell->menu == nullptr) {
        return false;
    }
    // The position is the frame's own screen position, passed as the
    // window-relative position PopupMenu takes, as the wxDragon GUI did.
    return g_shell->frame->PopupMenu(g_shell->menu, at.x, at.y);
}

void open_settings_dialog(const SettingsDialog& dialog) {
    if (g_shell == nullptr || g_shell->settings != nullptr) {
        return;
    }
    g_shell->settings = new SettingsDialogWindow(g_shell->frame, dialog);
    g_shell->settings->Show(true);
}

void open_list_dialog(const ListDialog& dialog) {
    if (g_shell == nullptr || g_shell->list != nullptr) {
        return;
    }
    g_shell->list = new ListDialogWindow(g_shell->frame, dialog);
    g_shell->list->Show(true);
}

std::size_t dialog_handle(DialogKind dialog) {
    wxDialog* window = DialogOf(dialog);
    return window == nullptr ? 0 : reinterpret_cast<std::size_t>(window->GetHWND());
}

void raise_dialog(DialogKind dialog) {
    if (wxDialog* window = DialogOf(dialog)) {
        window->Raise();
    }
}

void focus_dialog(DialogKind dialog) {
    if (wxDialog* window = DialogOf(dialog)) {
        window->SetFocus();
    }
}

void shut_down() {
    if (g_shell == nullptr) {
        return;
    }
    g_shell->shutting_down = true;
    if (g_shell->modal != nullptr) {
        g_shell->modal->EndModal(wxID_CANCEL);
    }
    if (g_shell->tray != nullptr) {
        g_shell->tray->RemoveIcon();
    }
    for (wxDialog* dialog : {static_cast<wxDialog*>(g_shell->settings),
                             static_cast<wxDialog*>(g_shell->list)}) {
        if (dialog != nullptr) {
            dialog->Destroy();
        }
    }
    g_shell->settings = nullptr;
    g_shell->list = nullptr;
    if (g_shell->frame != nullptr) {
        g_shell->frame->Close(true);
        g_shell->frame = nullptr;
    }
    wxTheApp->ExitMainLoop();
}

} // namespace verbatim_gui
