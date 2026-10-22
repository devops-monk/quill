import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { shouldAbortWindowsWebView2RuntimeFallback } from "@/lib/app/windowsWebView2RuntimePolicy";

const template = readFileSync(resolve(process.cwd(), "src-tauri/windows/nsis/installer.nsi"), "utf8");

describe("Windows offline installer template", () => {
  it("routes supported legacy Windows users from generic installers to the fixed-runtime package", () => {
    expect(template).toContain("ManifestSupportedOS all");
    expect(template).toContain("!include WinVer.nsh");
    // Tauri 2.11 renders fixedRuntime as an empty NSIS install mode instead of
    // the literal "fixedRuntime" string.
    expect(template).toContain('!if "${INSTALLWEBVIEW2MODE}" != ""');
    expect(template).not.toContain('!if "${INSTALLWEBVIEW2MODE}" != "fixedRuntime"');
    expect(template).toContain("${If} ${IsWin7}");
    expect(template).toContain("${OrIf} ${IsWin2012R2}");
    expect(template).toContain('MessageBox MB_ICONSTOP|MB_YESNO|MB_DEFBUTTON1 "$(dbxWin7InstallerRequired)" IDYES dbx_open_win7_installer');
    expect(template).toContain("https://dl.dbxio.com/releases/v${VERSION}/DBX_${VERSION}_x64-win7-server2012r2-offline-setup.exe?v=${VERSION}");
    expect(template).toContain("SetErrorLevel 1633");
    expect(template).toContain("${OrIf} $PassiveMode = 1");
  });

  it("localizes the Windows 7 package guidance in every installer language", () => {
    expect(template.match(/LangString dbxWin7InstallerRequired/g)).toHaveLength(3);
    expect(template).toContain("This installer does not support Windows 7 or Windows Server 2012 R2.");
    expect(template).toContain("此安装包不支持 Windows 7 或 Windows Server 2012 R2。");
    expect(template).toContain("此安裝套件不支援 Windows 7 或 Windows Server 2012 R2。");
  });

  it.each([
    {
      name: "continues after installer failure when a compatible Runtime remains",
      input: { installerExitCode: 1, runtimeDetected: true, runtimeMeetsMinimum: true },
      expected: false,
    },
    {
      name: "aborts after installer failure when the Runtime is missing",
      input: { installerExitCode: 1, runtimeDetected: false, runtimeMeetsMinimum: false },
      expected: true,
    },
    {
      name: "aborts after installer failure when the Runtime is below the minimum",
      input: { installerExitCode: 1, runtimeDetected: true, runtimeMeetsMinimum: false },
      expected: true,
    },
    {
      name: "continues after a successful installer even before the registry refresh",
      input: { installerExitCode: 0, runtimeDetected: false, runtimeMeetsMinimum: false },
      expected: false,
    },
  ])("$name", ({ input, expected }) => {
    expect(shouldAbortWindowsWebView2RuntimeFallback(input)).toBe(expected);
  });

  it("binds the tested fallback inputs to the NSIS Runtime recheck contract", () => {
    expect(template).toContain(`!macro ShouldAbortWebView2OfflineInstall INSTALL_RESULT RUNTIME_VERSION MINIMUM_COMPARISON RESULT
  StrCpy \${RESULT} 0
  \${If} \${INSTALL_RESULT} <> 0
    \${If} \${RUNTIME_VERSION} == ""
      StrCpy \${RESULT} 1
    \${ElseIf} \${MINIMUM_COMPARISON} = 1
      StrCpy \${RESULT} 1
    \${EndIf}
  \${EndIf}
!macroend`);
    expect(template).toContain(`!insertmacro ReadWebView2RuntimeVersion $4
      \${If} $4 != ""
        !if "\${MINIMUMWEBVIEW2VERSION}" != ""
          \${VersionCompare} "\${MINIMUMWEBVIEW2VERSION}" "$4" $R0
        !endif
      \${EndIf}
    \${EndIf}
    !insertmacro ShouldAbortWebView2OfflineInstall $1 $4 $R0 $R1
    \${If} $R1 = 1
      Abort "$(webview2AbortError)"
    \${EndIf}`);
  });

  it("uses the same registry detection before and after Runtime installation", () => {
    const runtimeChecks = template.match(/!insertmacro ReadWebView2RuntimeVersion \$4/g) ?? [];

    expect(runtimeChecks).toHaveLength(2);
    expect(template).toContain('ReadRegStr ${RESULT} HKLM "SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\${WEBVIEW2APPGUID}" "pv"');
    expect(template).toContain('ReadRegStr ${RESULT} HKCU "SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\${WEBVIEW2APPGUID}" "pv"');
  });
});
