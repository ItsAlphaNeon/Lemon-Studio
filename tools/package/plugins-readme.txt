Lemon Studio's plugins, as VST3 and CLAP plugins for other DAWs
=================================================================

Lemon Studio itself has these plugins built in; this download is for other DAWs.
Copy the contents of the VST3 folder to your VST3 plugin folder, and the contents of
the CLAP folder to your CLAP plugin folder, then let your DAW scan for plugins.

Windows
  VST3: %LOCALAPPDATA%\Programs\Common\VST3   (for you only)
        C:\Program Files\Common Files\VST3     (for everyone; needs an administrator)
  CLAP: %LOCALAPPDATA%\Programs\Common\CLAP
        C:\Program Files\Common Files\CLAP

Linux
  VST3: ~/.vst3
  CLAP: ~/.clap

macOS
  VST3: ~/Library/Audio/Plug-Ins/VST3
  CLAP: ~/Library/Audio/Plug-Ins/CLAP
  The plugins are not notarized by Apple. After copying them, run in Terminal:
    xattr -dr com.apple.quarantine ~/Library/Audio/Plug-Ins/VST3/*.vst3 ~/Library/Audio/Plug-Ins/CLAP/*.clap
  or DAWs won't load them.

Lemon Studio is free software under the GNU General Public License, version 3 or later:
https://github.com/ItsAlphaNeon/Lemon-Studio
