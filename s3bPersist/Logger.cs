using System;
using System.Collections.Generic;
using System.Text;

namespace s3b
{
    public class Logger
    {

        protected static PersistBase _persist;


        public static PersistBase Persist
        {
            set { _persist = value; }
            get { return _persist; }
        }

        public static void log(string logType, string msg)
        {
            MessageLog L = new MessageLog();
            L.msg = msg;
            L.log_type = logType;
            L.event_time = DateTime.Now;

            Persist.insert(L);

            Console.WriteLine(L.ToString());
        }

        public static void error( string msg )
        {
            log("error", msg);
        }

        public static void error(Exception x)
        {
            log("error", x.Message);
            log("error", x.StackTrace);

            if (x.InnerException != null)
            {
                log("error", x.InnerException.Message);
                log("error", x.InnerException.StackTrace);
            }
        }

        public static void info(string msg)
        {
            log("info", msg);
        }

        public static void debug( string msg )
        {
           //log("debug", msg);
        }

        public static void warn(string msg)
        {
            log("warn", msg);
        }

        
    }
}
