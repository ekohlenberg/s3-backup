using System;
namespace s3b
{
    public class Restore : Job
    {
        public Restore()
        {
            name = "restore";
            jobs.Add(new Download());
           // jobs.Add(new Decrypt());
           // jobs.Add(new Decompress());
           // jobs.Add(new Expand());
        }


        public override bool run(Model args)
        { 
            bool success = true;
            foreach(Job job in jobs)
            {
                if (!success) continue;
                success = job.run(args);

            }
            
            return success;
        }
    }
}
