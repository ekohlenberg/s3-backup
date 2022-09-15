using System;
using System.Collections.Generic;
using System.Text;
using System.Reflection;

namespace s3b
{
    public class MessageLog : Model
    {
        public MessageLog()
        {
            tableName = "message_log";
        }

        public string log_type
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }


        public string msg
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime event_time
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public override string ToString()
        {
            return event_time.ToString() + " " + log_type + " " + msg;
        }

    }
}
