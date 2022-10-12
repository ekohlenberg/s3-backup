using System;
using System.Collections.Generic;
using System.Text;
using System.Diagnostics;

namespace s3b
{
    public class ProcExec
    {
        string cmd = string.Empty;
        string argTemplate = string.Empty;
        char[] delim = new char[] {' '};
        
        
        
        public delegate void OutputCallback( string[] tokens );
      

        public ProcExec(string cmd, string argTemplate)
        {
            this.cmd = cmd;
            this.argTemplate = argTemplate;
        }
        public int run(Model parameters, OutputCallback stdout, OutputCallback stderr)
        {
            
            ProcessStartInfo processStartInfo;
            Process process;

            Template template = new Template();

            
            processStartInfo = new ProcessStartInfo();
            processStartInfo.CreateNoWindow = true;
            processStartInfo.RedirectStandardOutput = true;
            processStartInfo.RedirectStandardError = false;
            processStartInfo.RedirectStandardInput = false;
            processStartInfo.UseShellExecute = false;
            processStartInfo.Arguments = template.eval(argTemplate, parameters);
            processStartInfo.FileName = cmd;

            Logger.info(cmd + " " + processStartInfo.Arguments);

            process = new Process();
            process.StartInfo = processStartInfo;
            // enable raising events because Process does not raise events by default
            process.EnableRaisingEvents = true;
            // attach the event handler for OutputDataReceived before starting the process
            process.OutputDataReceived += new DataReceivedEventHandler
            (
                delegate (object sender, DataReceivedEventArgs e)
                {
                    // append the new data to the data already read-in
                    if (e.Data == null) return;
                    if (e.Data.Length == 0) return;
                    string[] parts = e.Data.Split(delim, StringSplitOptions.RemoveEmptyEntries);

                    if (stdout != null)
                    {
                        stdout(parts);
                        
                    }                 
                    Console.WriteLine(e.Data);
                }
            );

            process.ErrorDataReceived += new DataReceivedEventHandler
            (
                delegate (object sender, DataReceivedEventArgs e)
                {
                    if (e.Data == null) return;
                    if (e.Data.Length == 0) return;
                    string[] parts = e.Data.Split(delim, StringSplitOptions.RemoveEmptyEntries);

                    if (stderr != null)
                    {
                        stderr(parts);
                    }   
                    
                    Console.WriteLine(e.Data);
                }
            );
            // start the process
            // then begin asynchronously reading the output
            // then wait for the process to exit
            // then cancel asynchronously reading the output
            process.Start();
            process.BeginOutputReadLine();
            process.WaitForExit();
            process.CancelOutputRead();
            
       

            return process.ExitCode;
        }
    }
}
