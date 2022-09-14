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

        public string stdout = string.Empty;
        public string stderr = string.Empty;

        public ProcExec(string cmd, string argTemplate)
        {
            this.cmd = cmd;
            this.argTemplate = argTemplate;
        }
        public int run(Model parameters)
        {
            StringBuilder outputBuilder;
            StringBuilder errorBuilder;
            ProcessStartInfo processStartInfo;
            Process process;

            Template template = new Template(argTemplate);

            outputBuilder = new StringBuilder();
            errorBuilder = new StringBuilder();

            processStartInfo = new ProcessStartInfo();
            processStartInfo.CreateNoWindow = true;
            processStartInfo.RedirectStandardOutput = false;
            processStartInfo.RedirectStandardError = false;
            processStartInfo.RedirectStandardInput = false;
            processStartInfo.UseShellExecute = true;
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
                    outputBuilder.Append(e.Data);
                    Console.Write(e.Data);
                }
            );

            process.ErrorDataReceived += new DataReceivedEventHandler
            (
                delegate (object sender, DataReceivedEventArgs e)
                {
                    // append the new data to the data already read-in
                    errorBuilder.Append(e.Data);
                    Console.Write(e.Data);
                }
            );
            // start the process
            // then begin asynchronously reading the output
            // then wait for the process to exit
            // then cancel asynchronously reading the output
            process.Start();
            //process.BeginOutputReadLine();
            process.WaitForExit();
           // process.CancelOutputRead();
            
            // use the output
            stdout = outputBuilder.ToString();
            if (stdout.Length > 0) Logger.info(stdout);
            stderr = errorBuilder.ToString();
            if (stderr.Length > 0) Logger.error(stdout);

            return process.ExitCode;
        }
    }
}
