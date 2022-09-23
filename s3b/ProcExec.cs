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

        public List<string> stdout = new List<string>();
        public List<string> stderr = new List<string>();

        public ProcExec(string cmd, string argTemplate)
        {
            this.cmd = cmd;
            this.argTemplate = argTemplate;
        }
        public int run(Model parameters)
        {
            
            ProcessStartInfo processStartInfo;
            Process process;

            Template template = new Template(argTemplate);

            
            processStartInfo = new ProcessStartInfo();
            processStartInfo.CreateNoWindow = true;
            processStartInfo.RedirectStandardOutput = true;
            processStartInfo.RedirectStandardError = false;
            processStartInfo.RedirectStandardInput = false;
            processStartInfo.UseShellExecute = false;
            processStartInfo.Arguments = template.eval(parameters);
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
                    
                    stdout.Add(e.Data);                    
                    Console.WriteLine(e.Data);
                }
            );

            process.ErrorDataReceived += new DataReceivedEventHandler
            (
                delegate (object sender, DataReceivedEventArgs e)
                {
                    // append the new data to the data already read-in
                    stderr.Add(e.Data);
                    
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
            
            // use the output
            
            if (stdout.Count > 0) Logger.info(stdout);
            
            if (stderr.Count > 0) Logger.error(stdout);

            return process.ExitCode;
        }
    }
}
