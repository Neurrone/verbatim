<#
.SYNOPSIS
Pins the guest's default audio render device (all three roles: console,
multimedia, communications) to the render endpoint whose friendly name
matches -FriendlyNameMatch, and verifies the pin actually took effect.

.DESCRIPTION
Why this needs to be explicit rather than trusting "VB-CABLE is the only
render endpoint so it is already default": that only holds headlessly. The
moment a Remote Desktop session with audio redirection connects (cargo
xtask vm connect, or any Enhanced Session), Windows adds a "Remote Audio"
render endpoint to the guest and can switch the session's default to it —
silently routing OneCore's speech there instead of to VB-CABLE, so a
`cargo xtask vm test --record` running at the same time would produce a
video with a silent audio track and no error anywhere to explain why. This
script makes the pin explicit and durable, re-assertable on demand, instead
of relying on incidental single-device defaulting.

Windows ships no cmdlet for setting the default audio endpoint. This uses
the same undocumented-but-long-stable mechanism essentially every "change
default audio device" tool has used since Windows Vista
(IPolicyConfig::SetDefaultEndpoint), reached from the documented, public
Core Audio API (IMMDeviceEnumerator / IMMDevice) so the only undocumented
surface touched is the one call with no documented alternative — every
other interface declared below (IMMDeviceEnumerator, IMMDeviceCollection,
IMMDevice) is public, Microsoft-documented COM (mmdeviceapi.h), declared
only as deep as the methods actually called (a standard, safe COM interop
pattern: truncating a vtable-ordered interface definition after the last
member used is safe as long as nothing past it is ever invoked). The
result is independently verified afterward with GetDefaultAudioEndpoint —
also documented, public API — for all three roles, so a mistaken
assumption anywhere in the one undocumented call fails this script loudly
instead of silently leaving the wrong device pinned.

The target endpoint is resolved from the registry
(MMDevices\Audio\Render\{guid}\Properties) by matching FriendlyNameMatch
against every string-valued property under that key, rather than a single
named property such as PKEY_Device_FriendlyName specifically: that
property's numbered id has differed across documented references, while
every render endpoint's Properties key has exactly one human-readable
string value containing the device's name, so matching by value sidesteps
needing to know which numbered property it is.

Called two ways, kept in lockstep by hand rather than sharing code (the
same tradeoff `verbatim_config::Settings::for_e2e`'s doc comment makes for
the host/guest boundary elsewhere in this repository): once with its
content embedded as a here-string inside
vm/scripts/Initialize-VerbatimHarness.ps1 (Packer's powershell provisioner
uploads and runs exactly one script file, so a sibling file cannot be
referenced directly), pinning the baseline at image-build time; and again
via `xtask/src/vm/recording.rs`, which embeds this exact file's content at
Rust compile time (`include_str!`) and sends it over PowerShell Direct
before every `cargo xtask vm test --record` run — re-pinning at record
time is deliberate: it is what survives a `cargo xtask vm connect` session
having switched the default to Remote Audio since the last restore.

.PARAMETER FriendlyNameMatch
A `-like` pattern matched against each active render endpoint's friendly
name. Defaults to "*VB-Audio*".
#>
[CmdletBinding()]
param(
    [string]$FriendlyNameMatch = "*VB-Audio*"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-AudioPinFact {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host "audio-pin: $Message"
}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

namespace VerbatimAudioPin
{
    internal enum EDataFlow { eRender = 0 }
    internal enum ERole { eConsole = 0, eMultimedia = 1, eCommunications = 2 }

    // Public, documented Core Audio API (mmdeviceapi.h) — stable since
    // Windows Vista. Interfaces below are truncated after the last method
    // this script actually calls; see this file's own header comment.
    [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IMMDeviceEnumerator
    {
        int EnumAudioEndpoints(EDataFlow dataFlow, int dwStateMask, out IMMDeviceCollection ppDevices);
        int GetDefaultAudioEndpoint(EDataFlow dataFlow, ERole role, out IMMDevice ppEndpoint);
    }

    [Guid("0BD7A1BE-7A1A-44DB-8397-CC5392387B5E"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IMMDeviceCollection
    {
        int GetCount(out int pcDevices);
        int Item(int nDevice, out IMMDevice ppDevice);
    }

    [Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IMMDevice
    {
        int Activate(ref Guid iid, int dwClsCtx, IntPtr pActivationParams, [MarshalAs(UnmanagedType.IUnknown)] out object ppInterface);
        int OpenPropertyStore(int stgmAccess, out IntPtr ppProperties);
        int GetId([MarshalAs(UnmanagedType.LPWStr)] out string ppstrId);
    }

    [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
    internal class MMDeviceEnumeratorComObject { }

    // Undocumented; Microsoft publishes no alternative for setting the
    // default audio endpoint. This exact vtable ordering is the one nearly
    // every "change default audio device" tool for Windows Vista through
    // 11 has used for over a decade.
    [Guid("F8679F50-850A-41CF-9C72-430F290290C8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IPolicyConfig
    {
        int GetMixFormat(string pszDeviceName, IntPtr ppFormat);
        int GetDeviceFormat(string pszDeviceName, bool bDefault, IntPtr ppFormat);
        int ResetDeviceFormat(string pszDeviceName);
        int SetDeviceFormat(string pszDeviceName, IntPtr pEndpointFormat, IntPtr mixFormat);
        int GetProcessingPeriod(string pszDeviceName, bool bDefault, out long hnsDefaultDevicePeriod, out long hnsMinimumDevicePeriod);
        int SetProcessingPeriod(string pszDeviceName, ref long hnsDevicePeriod);
        int GetShareMode(string pszDeviceName, IntPtr pMode);
        int SetShareMode(string pszDeviceName, IntPtr mode);
        int GetPropertyValue(string pszDeviceName, IntPtr key, IntPtr pv);
        int SetPropertyValue(string pszDeviceName, IntPtr key, IntPtr pv);
        int SetDefaultEndpoint(string pszDeviceName, ERole role);
        int SetEndpointVisibility(string pszDeviceName, bool bVisible);
    }

    [ComImport, Guid("870AF99C-171D-4F9E-AF0D-E63DF40C2BC9")]
    internal class PolicyConfigClientComObject { }

    public static class DefaultRenderDevicePin
    {
        // guidSubstring: the target endpoint's registry-key GUID (with
        // braces), already resolved by this file's PowerShell body against
        // the registry. IMMDevice.GetId()'s returned id string always ends
        // in ".{that same GUID}", so matching on it — rather than repeating
        // a friendly-name lookup through IPropertyStore/PROPVARIANT here —
        // avoids a second, riskier COM property read.
        public static string Pin(string guidSubstring)
        {
            var enumerator = (IMMDeviceEnumerator)new MMDeviceEnumeratorComObject();
            IMMDeviceCollection collection;
            int hr = enumerator.EnumAudioEndpoints(EDataFlow.eRender, /* DEVICE_STATE_ACTIVE */ 1, out collection);
            if (hr != 0) throw new InvalidOperationException("EnumAudioEndpoints failed: 0x" + hr.ToString("X8"));

            int count;
            collection.GetCount(out count);

            string targetId = null;
            for (int i = 0; i < count; i++)
            {
                IMMDevice device;
                collection.Item(i, out device);
                string id;
                device.GetId(out id);
                if (id.IndexOf(guidSubstring, StringComparison.OrdinalIgnoreCase) >= 0)
                {
                    targetId = id;
                    break;
                }
            }
            if (targetId == null)
                throw new InvalidOperationException("no active render endpoint id contains '" + guidSubstring + "'");

            var policyConfig = (IPolicyConfig)new PolicyConfigClientComObject();
            foreach (ERole role in new[] { ERole.eConsole, ERole.eMultimedia, ERole.eCommunications })
            {
                int setHr = policyConfig.SetDefaultEndpoint(targetId, role);
                if (setHr != 0)
                    throw new InvalidOperationException("SetDefaultEndpoint(" + role + ") failed: 0x" + setHr.ToString("X8"));
            }

            foreach (ERole role in new[] { ERole.eConsole, ERole.eMultimedia, ERole.eCommunications })
            {
                IMMDevice defaultDevice;
                int getHr = enumerator.GetDefaultAudioEndpoint(EDataFlow.eRender, role, out defaultDevice);
                if (getHr != 0)
                    throw new InvalidOperationException("GetDefaultAudioEndpoint(" + role + ") failed: 0x" + getHr.ToString("X8"));
                string defaultId;
                defaultDevice.GetId(out defaultId);
                if (!string.Equals(defaultId, targetId, StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException("verification failed for role " + role + ": default is '" + defaultId + "', expected '" + targetId + "'");
            }

            return targetId;
        }
    }
}
"@

$targetDevice = Get-ChildItem -Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render" -ErrorAction SilentlyContinue |
    Where-Object {
        $propertiesPath = Join-Path $_.PSPath "Properties"
        if (-not (Test-Path -LiteralPath $propertiesPath)) { return $false }
        $properties = Get-ItemProperty -LiteralPath $propertiesPath -ErrorAction SilentlyContinue
        if (-not $properties) { return $false }
        foreach ($property in $properties.PSObject.Properties) {
            if ($property.Value -is [string] -and $property.Value -like $FriendlyNameMatch) {
                return $true
            }
        }
        return $false
    } |
    Select-Object -First 1

if (-not $targetDevice) {
    throw "no render endpoint under MMDevices\Audio\Render has a friendly name matching '$FriendlyNameMatch'"
}
$targetGuid = $targetDevice.PSChildName
Write-AudioPinFact "target render endpoint guid: $targetGuid (matched '$FriendlyNameMatch')"

$pinnedId = [VerbatimAudioPin.DefaultRenderDevicePin]::Pin($targetGuid)
Write-AudioPinFact "default render endpoint pinned and verified for all three roles: $pinnedId"
