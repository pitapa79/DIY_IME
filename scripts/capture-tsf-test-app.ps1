param(
    [Parameter(Mandatory)][string]$StagingDirectory,
    [Parameter(Mandatory)][string[]]$AppProcessName,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [Parameter(Mandatory)][ValidatePattern('^[a-z0-9-]+$')][string]$CaptureName
)
$ErrorActionPreference = 'Stop'
$expectedDll = [IO.Path]::GetFullPath((Join-Path $StagingDirectory 'diy_ime2_tsf.dll'))
$output = (Resolve-Path -LiteralPath $OutputDirectory).Path
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.IO;
using System.Diagnostics;
using System.Collections.Generic;
public static class StageBDebugCapture {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateEventW(IntPtr security, bool manual, bool initial, string name);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateFileMappingW(IntPtr file, IntPtr security, uint protection, uint high, uint low, string name);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr MapViewOfFile(IntPtr mapping, uint access, uint high, uint low, UIntPtr size);
    [DllImport("kernel32.dll")] static extern bool UnmapViewOfFile(IntPtr view);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern bool SetEvent(IntPtr handle);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle,uint ms);
    public static void Run(string stop, string log, string expectedDll, string[] appNames) {
        var verified = new Dictionary<int, Process>();
        var buffer = new byte[4092];
        IntPtr mapping=IntPtr.Zero, view=IntPtr.Zero, ready=IntPtr.Zero, data=IntPtr.Zero;
        try {
            mapping=CreateFileMappingW(new IntPtr(-1),IntPtr.Zero,4,0,4096,"DBWIN_BUFFER");
            if(mapping==IntPtr.Zero || Marshal.GetLastWin32Error()==183)
                throw new Exception("Debug output capture is already in use or unavailable.");
            view=MapViewOfFile(mapping,4,0,0,(UIntPtr)4096);
            ready=CreateEventW(IntPtr.Zero,false,false,"DBWIN_BUFFER_READY");
            data=CreateEventW(IntPtr.Zero,false,false,"DBWIN_DATA_READY");
            if(view==IntPtr.Zero || ready==IntPtr.Zero || data==IntPtr.Zero)
                throw new Exception("Cannot open debug output capture.");
            Console.WriteLine("Fixed Stage B diagnostic capture is ready.");
            while(!File.Exists(stop)) {
                SetEvent(ready);
                uint outcome=WaitForSingleObject(data,1000);
                if(outcome==258) continue;
                if(outcome!=0) throw new Exception("Debug event wait failed.");
                int pid=Marshal.ReadInt32(view);
                Marshal.Copy(IntPtr.Add(view,4),buffer,0,buffer.Length);
                int end=Array.IndexOf(buffer,(byte)0);
                if(end<0 || end>256) continue;
                string text=Encoding.ASCII.GetString(buffer,0,end);
                if(text.StartsWith("DIY IME Stage B: ",StringComparison.Ordinal)
                    && VerifyProcess(pid, expectedDll, appNames, verified)) {
                    string line="app="+verified[pid].ProcessName+" pid="+pid+" "+text;
                    File.AppendAllText(log,line+Environment.NewLine);
                }
            }
        } finally {
            foreach (var process in verified.Values) process.Dispose();
            if(view!=IntPtr.Zero) UnmapViewOfFile(view);
            foreach(IntPtr handle in new[]{mapping,ready,data})
                if(handle!=IntPtr.Zero) CloseHandle(handle);
        }
    }

    private static bool VerifyProcess(int pid, string expectedDll, string[] appNames,
        Dictionary<int, Process> verified) {
        if (verified.TryGetValue(pid, out var existing)) {
            if (!existing.HasExited) return true;
            verified.Remove(pid);
            existing.Dispose();
        }
        Process process = null;
        try {
            process = Process.GetProcessById(pid);
            if (!Array.Exists(appNames, name =>
                string.Equals(name, process.ProcessName, StringComparison.OrdinalIgnoreCase))) {
                return false;
            }
            foreach (ProcessModule module in process.Modules) {
                if (string.Equals(expectedDll, module.FileName, StringComparison.OrdinalIgnoreCase)) {
                    verified.Add(pid, process);
                    process = null;
                    return true;
                }
            }
            return false;
        } catch (ArgumentException) {
            return false;
        } catch (InvalidOperationException) {
            return false;
        } catch (System.ComponentModel.Win32Exception) {
            return false;
        } finally {
            if (process != null) process.Dispose();
        }
    }
}
'@
[StageBDebugCapture]::Run(
    (Join-Path $output "$CaptureName.stop"),
    (Join-Path $output "$CaptureName.log"),
    $expectedDll,
    $AppProcessName)
